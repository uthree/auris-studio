//! VST3 host implementation

use crate::{
    audio::AudioConfig,
    error::{Error, Result},
    plugin::{Plugin, PluginInfo, PluginInternal},
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Ceiling on the output channel count this host will size meters and buffers from.
///
/// Matches the isolation layer's own wire limit. The channel count of an isolated plugin is
/// whatever the helper says it is, and this boundary sizes an allocation from it directly, so
/// it defends itself rather than trusting the peer to have clamped first.
const MAX_OUTPUT_CHANNELS: usize = 256;

/// Turn a reported output channel count into one this host can size buffers from: fall back
/// to stereo when the plugin reports none, and never exceed [`MAX_OUTPUT_CHANNELS`].
fn clamp_output_channels(reported: i32) -> usize {
    match reported {
        n if n <= 0 => 2,
        n => (n as usize).min(MAX_OUTPUT_CHANNELS),
    }
}

/// VST3 host instance
pub struct Vst3Host {
    /// Audio configuration
    pub(crate) config: AudioConfig,
    /// Custom plugin scan paths
    pub(crate) custom_paths: Vec<PathBuf>,
    /// Whether to use process isolation for plugin loading
    pub(crate) use_process_isolation: bool,
    /// Whether to scan default system paths for plugins
    pub(crate) scan_default_paths: bool,
    /// Explicit path to the isolation helper binary (overrides the heuristic search).
    pub(crate) helper_path: Option<PathBuf>,
    /// How long to wait for an isolated helper response before declaring a timeout.
    pub(crate) response_timeout: std::time::Duration,
    /// Whether an isolated plugin auto-respawns + retries on a crash/hang (control plane only).
    pub(crate) auto_recover_plugins: bool,
    /// Max respawn+retry cycles per command when auto-recover is on.
    pub(crate) auto_recover_max_retries: u32,
    /// Per-plugin timeout for the crash-resistant discovery probe ([`Self::discover_plugins_safe`]).
    pub(crate) probe_timeout: std::time::Duration,
}

impl Vst3Host {
    /// Create a new VST3 host with default settings.
    ///
    /// Discovery scans the standard system VST3 directories (consistent with
    /// [`Vst3Host::default`]). For explicit control use [`Vst3Host::builder`]; the builder
    /// does **not** scan system paths unless you opt in with
    /// [`Vst3HostBuilder::scan_default_paths`].
    pub fn new() -> Result<Self> {
        Self::builder().scan_default_paths().build()
    }

    /// Create a new VST3 host builder
    pub fn builder() -> Vst3HostBuilder {
        Vst3HostBuilder::default()
    }

    /// Add a custom path to scan for VST3 plugins
    pub fn add_scan_path<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(Error::Other(format!(
                "Path does not exist: {}",
                path.display()
            )));
        }
        self.custom_paths.push(path.to_path_buf());
        Ok(())
    }

    /// Discover VST3 plugins in configured scan paths.
    ///
    /// # This can take your process down
    ///
    /// Every candidate is instantiated **in this process** to read its metadata, so a plugin that
    /// aborts, segfaults, or throws a C++ exception through the Rust frames during its own
    /// initialisation kills the host — there is nothing this function can catch. That is not
    /// hypothetical: a licensed Waves plugin in a normal `/Library/Audio/Plug-Ins/VST3` aborts
    /// with "Rust cannot catch foreign exceptions" during its license check.
    ///
    /// Prefer [`Self::discover_plugins_safe`], which introspects each plugin in a short-lived
    /// child process and reports the casualties as skips instead of dying. Use this one only when
    /// you control which plugins are present.
    pub fn discover_plugins(&mut self) -> Result<Vec<PluginInfo>> {
        let mut all_paths = self.custom_paths.clone();

        // Add system paths if enabled
        if self.scan_default_paths {
            all_paths.extend(crate::discovery::scan_standard_paths());
        }

        // Scan directories for VST3 plugins
        let plugin_paths = crate::discovery::scan_directories(&all_paths)?;

        // Get plugin info for each found plugin
        let mut plugins = Vec::new();
        for path in plugin_paths {
            match crate::discovery::get_plugin_info(&path) {
                Ok(info) => plugins.push(info),
                Err(e) => {
                    log::warn!("Failed to get info for plugin {}: {}", path.display(), e);
                    // Continue with other plugins
                }
            }
        }

        Ok(plugins)
    }

    /// List VST3 bundle paths in the configured scan locations **without loading them**.
    ///
    /// Fast and safe: unlike [`Self::discover_plugins`] (which loads and initializes
    /// every plugin to read its metadata, and can be slow or crash-prone in-process),
    /// this only walks the filesystem. Use it when you just need the list of available
    /// `.vst3` paths (e.g. to populate a picker) and will load on demand.
    pub fn scan_plugin_paths(&self) -> Vec<std::path::PathBuf> {
        let mut all_paths = self.custom_paths.clone();
        if self.scan_default_paths {
            all_paths.extend(crate::discovery::scan_standard_paths());
        }
        crate::discovery::scan_directories(&all_paths).unwrap_or_default()
    }

    /// Discover VST3 plugins, reporting progress through a callback.
    ///
    /// The callback receives [`DiscoveryProgress`] events: one `Started` at the
    /// beginning, a `Found` or `Error` per candidate, and a final `Completed`.
    /// Returns the successfully-inspected plugins, same as [`Self::discover_plugins`].
    pub fn discover_plugins_with_callback<F>(
        &mut self,
        mut on_progress: F,
    ) -> Result<Vec<PluginInfo>>
    where
        F: FnMut(DiscoveryProgress),
    {
        let mut all_paths = self.custom_paths.clone();

        if self.scan_default_paths {
            all_paths.extend(crate::discovery::scan_standard_paths());
        }

        let plugin_paths = crate::discovery::scan_directories(&all_paths)?;
        let total = plugin_paths.len();

        on_progress(DiscoveryProgress::Started {
            total_plugins: total,
        });

        let mut plugins = Vec::new();
        for (index, path) in plugin_paths.into_iter().enumerate() {
            match crate::discovery::get_plugin_info(&path) {
                Ok(info) => {
                    on_progress(DiscoveryProgress::Found {
                        plugin: info.clone(),
                        current: index + 1,
                        total,
                    });
                    plugins.push(info);
                }
                Err(e) => {
                    log::warn!("Failed to get info for plugin {}: {}", path.display(), e);
                    on_progress(DiscoveryProgress::Error {
                        path: path.display().to_string(),
                        error: e.to_string(),
                    });
                }
            }
        }

        on_progress(DiscoveryProgress::Completed {
            total_found: plugins.len(),
        });

        Ok(plugins)
    }

    /// Crash-resistantly discover plugins in the configured scan paths.
    ///
    /// Unlike [`Self::discover_plugins`] — which instantiates each plugin **in-process**
    /// to read its metadata, so a single plugin that `abort()`s or makes a pure-virtual
    /// call during init takes down the whole host — this introspects every plugin in a
    /// throwaway child process (`vst3-host-probe`). A plugin that crashes kills only that
    /// child; the scan completes and returns the plugins it could introspect, recording
    /// the skipped ones (and why) in the returned
    /// [`SafeDiscoveryReport`](crate::discovery::SafeDiscoveryReport).
    ///
    /// Trade-off: this spawns one probe process per plugin, so it is slower than the
    /// in-process path. Use it to safely scan an untrusted folder; keep
    /// [`Self::discover_plugins`] for speed when you trust the plugins.
    ///
    /// The probe timeout per plugin defaults to
    /// [`DEFAULT_PROBE_TIMEOUT`](crate::discovery::DEFAULT_PROBE_TIMEOUT); override it with
    /// [`Vst3HostBuilder::probe_timeout`].
    ///
    /// If the `vst3-host-probe` binary cannot be located the scan never runs; the report is
    /// empty and says why in [`SafeDiscoveryReport::error`](crate::discovery::SafeDiscoveryReport::error).
    pub fn discover_plugins_safe(&self) -> crate::discovery::SafeDiscoveryReport {
        let mut all_paths = self.custom_paths.clone();
        if self.scan_default_paths {
            all_paths.extend(crate::discovery::scan_standard_paths());
        }
        crate::discovery::discover_plugins_safe(&all_paths, self.probe_timeout)
    }

    /// Load a VST3 plugin
    pub fn load_plugin<P: AsRef<Path>>(&mut self, path: P) -> Result<Plugin> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(Error::PluginNotFound(path.display().to_string()));
        }

        if self.use_process_isolation {
            self.load_plugin_isolated(path, None)
        } else {
            self.load_plugin_internal(path, None)
        }
    }

    /// Load a particular audio class from a VST3 bundle.
    ///
    /// `class_id` may be either a current class id exported by the factory or a retired id
    /// mapped to its replacement by the bundle's validated `moduleinfo.json`. This is useful
    /// when restoring a session whose plugin id predates a vendor's UID migration.
    pub fn load_plugin_class<P: AsRef<Path>>(&mut self, path: P, class_id: &str) -> Result<Plugin> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(Error::PluginNotFound(path.display().to_string()));
        }
        crate::internal::utils::parse_class_uid(class_id).ok_or_else(|| {
            Error::PluginLoadFailed(
                "class id must be exactly 32 hexadecimal characters".to_string(),
            )
        })?;

        if self.use_process_isolation {
            self.load_plugin_isolated(path, Some(class_id))
        } else {
            self.load_plugin_internal(path, Some(class_id))
        }
    }

    /// Probe whether a plugin loads safely, **without risking the host process** — it is
    /// loaded in an isolated helper, so a crash is contained. This is the "validate
    /// plugins" operation a scanner uses to blacklist bad plugins.
    ///
    /// Requires the `process-isolation` feature.
    #[cfg(feature = "process-isolation")]
    pub fn probe_plugin<P: AsRef<Path>>(&self, path: P) -> ProbeResult {
        use crate::process_isolation::{HostCommand, HostResponse, PluginHostProcess};

        let path = path.as_ref();
        if !path.exists() {
            return ProbeResult::Failed("plugin path does not exist".to_string());
        }
        let mut process =
            match PluginHostProcess::new(self.helper_path.clone(), self.response_timeout) {
                Ok(p) => p,
                Err(e) => return ProbeResult::Failed(format!("helper unavailable: {e}")),
            };
        match process.send_command(HostCommand::LoadPlugin {
            path: path.display().to_string(),
            sample_rate: self.config.sample_rate,
            block_size: self.config.block_size as u32,
            tempo: self.config.tempo,
            time_sig_numerator: self.config.time_sig_numerator,
            time_sig_denominator: self.config.time_sig_denominator,
            class_id: None,
        }) {
            Ok(HostResponse::PluginInfo { .. }) => ProbeResult::Ok,
            Ok(HostResponse::Error { message }) => ProbeResult::Failed(message),
            Ok(_) => ProbeResult::Failed("unexpected response from helper".to_string()),
            Err(e) if e.to_lowercase().contains("crash") => ProbeResult::Crashed,
            Err(e) if e.to_lowercase().contains("timed out") => ProbeResult::TimedOut,
            Err(e) => ProbeResult::Failed(e),
        }
    }

    /// Load a plugin in-process
    fn load_plugin_internal(&mut self, path: &Path, class_id: Option<&str>) -> Result<Plugin> {
        // Load the plugin implementation directly - it will handle path resolution
        let mut plugin_impl = match class_id {
            Some(class_id) => crate::internal::plugin_impl::PluginImpl::load_class(path, class_id)?,
            None => crate::internal::plugin_impl::PluginImpl::load(path)?,
        };

        // Apply the builder's audio config (sample rate / block size) so the plugin actually
        // processes at the requested settings, not the internal defaults.
        plugin_impl.set_audio_config(self.config.sample_rate, self.config.block_size);

        // Thread the configured transport into the plugin's host ProcessContext so
        // tempo-synced DSP sees the host tempo / time signature.
        plugin_impl.set_transport(
            self.config.tempo,
            self.config.time_sig_numerator,
            self.config.time_sig_denominator,
        );

        // Get the updated info from the plugin implementation (has_gui might have been updated)
        let updated_info = plugin_impl.info.clone();
        let compatibility = plugin_impl.compatibility.clone();

        // Size meters to the plugin's real output channel count (bus-aware), not a stereo
        // assumption; fall back to 2 only when the plugin reports no output channels.
        let output_channels = match plugin_impl.output_channel_count() {
            0 => 2,
            n => n,
        };

        let plugin = Plugin {
            info: updated_info,
            compatibility,
            is_processing: false,
            realtime_owned: false,
            sample_rate: self.config.sample_rate,
            block_size: self.config.block_size,
            audio_levels: Arc::new(Mutex::new(crate::audio::AudioLevels::new(output_channels))),
            parameter_change_callback: None,
            audio_callback: None,
            internal: Some(Box::new(plugin_impl)),
        };

        Ok(plugin)
    }

    /// Load a plugin in an isolated process
    fn load_plugin_isolated(&mut self, path: &Path, class_id: Option<&str>) -> Result<Plugin> {
        use crate::process_isolation::{HostCommand, HostResponse, PluginHostProcess};

        // Create and start the isolated plugin process
        let mut process =
            PluginHostProcess::new(self.helper_path.clone(), self.response_timeout)
                .map_err(|e| Error::Other(format!("Failed to create isolated process: {}", e)))?;

        // Load the plugin in the isolated process
        let response = process
            .send_command(HostCommand::LoadPlugin {
                path: path.display().to_string(),
                sample_rate: self.config.sample_rate,
                block_size: self.config.block_size as u32,
                tempo: self.config.tempo,
                time_sig_numerator: self.config.time_sig_numerator,
                time_sig_denominator: self.config.time_sig_denominator,
                class_id: class_id.map(str::to_owned),
            })
            .map_err(|e| Error::Other(format!("Failed to load plugin in isolation: {}", e)))?;

        // Verify the plugin loaded successfully. Metadata comes straight from the helper's
        // accurate introspection, so the isolated path matches the in-process one.
        let (loaded_info, compatibility, output_channels) = match response {
            HostResponse::PluginInfo {
                vendor,
                name,
                version,
                category,
                uid,
                has_gui,
                audio_inputs,
                audio_outputs,
                output_channels,
                has_midi_input,
                has_midi_output,
                compatibility,
            } => {
                let info = PluginInfo {
                    path: path.to_path_buf(),
                    name,
                    vendor,
                    version,
                    category,
                    uid,
                    has_gui,
                    audio_inputs: audio_inputs as u32,
                    audio_outputs: audio_outputs as u32,
                    has_midi_input,
                    has_midi_output,
                };
                let channels = clamp_output_channels(output_channels);
                (info, compatibility, channels)
            }
            HostResponse::Error { message } => {
                return Err(Error::Other(format!("Failed to load plugin: {}", message)));
            }
            _ => {
                return Err(Error::Other(
                    "Unexpected response from helper process".to_string(),
                ));
            }
        };

        // Create the isolated plugin implementation
        let plugin_impl = crate::internal::isolated_plugin_impl::IsolatedPluginImpl::new(
            process,
            loaded_info.clone(),
            self.config.sample_rate,
            self.config.block_size,
            self.config.tempo,
            self.config.time_sig_numerator,
            self.config.time_sig_denominator,
            output_channels,
            self.helper_path.clone(),
            self.response_timeout,
            self.auto_recover_plugins,
            self.auto_recover_max_retries,
        );

        let plugin = Plugin {
            info: loaded_info,
            compatibility,
            is_processing: false,
            realtime_owned: false,
            sample_rate: self.config.sample_rate,
            block_size: self.config.block_size,
            audio_levels: Arc::new(Mutex::new(crate::audio::AudioLevels::new(output_channels))),
            parameter_change_callback: None,
            audio_callback: None,
            internal: Some(Box::new(plugin_impl)),
        };

        Ok(plugin)
    }

    /// Get audio configuration
    pub fn config(&self) -> &AudioConfig {
        &self.config
    }
}

impl Default for Vst3Host {
    fn default() -> Self {
        Self {
            config: AudioConfig::default(),
            custom_paths: Vec::new(),
            use_process_isolation: false,
            scan_default_paths: true,
            helper_path: None,
            response_timeout: crate::process_isolation::DEFAULT_RESPONSE_TIMEOUT,
            auto_recover_plugins: false,
            auto_recover_max_retries: 1,
            probe_timeout: crate::discovery::DEFAULT_PROBE_TIMEOUT,
        }
    }
}

/// Builder for VST3 host configuration
///
/// All fields default to their type defaults; notably `scan_default_paths` defaults to
/// `false`, requiring explicit opt-in (unlike `Vst3Host`, which defaults it to `true`).
#[derive(Default)]
pub struct Vst3HostBuilder {
    config: AudioConfig,
    custom_paths: Vec<PathBuf>,
    use_process_isolation: bool,
    scan_default_paths: bool,
    helper_path: Option<PathBuf>,
    response_timeout: Option<std::time::Duration>,
    auto_recover_plugins: bool,
    auto_recover_max_retries: Option<u32>,
    probe_timeout: Option<std::time::Duration>,
}

impl Vst3HostBuilder {
    /// Set the sample rate
    pub fn sample_rate(mut self, rate: f64) -> Self {
        self.config.sample_rate = rate;
        self
    }

    /// Set the block size
    pub fn block_size(mut self, size: usize) -> Self {
        self.config.block_size = size;
        self
    }

    /// Set the number of input channels
    pub fn input_channels(mut self, channels: usize) -> Self {
        self.config.input_channels = channels;
        self
    }

    /// Set the number of output channels
    pub fn output_channels(mut self, channels: usize) -> Self {
        self.config.output_channels = channels;
        self
    }

    /// Set the transport tempo (beats per minute) advertised to plugins in the host
    /// `ProcessContext`. Drives tempo-synced DSP (LFOs, synced delays, arpeggiators).
    /// Defaults to `120.0`. Non-finite or non-positive values are ignored (a tempo of 0 or
    /// less would freeze/reverse the derived musical playhead), keeping the previous tempo.
    pub fn tempo(mut self, bpm: f64) -> Self {
        if bpm.is_finite() && bpm > 0.0 {
            self.config.tempo = bpm;
        }
        self
    }

    /// Set the transport time signature advertised to plugins in the host
    /// `ProcessContext` (`num`/`den`, e.g. `4, 4`). Defaults to `4/4`.
    ///
    /// Validated by [`Self::build`] against the same rule every runtime entry point applies
    /// (see [`Plugin::set_time_signature`](crate::Plugin::set_time_signature)): `num` positive
    /// and `den` one of `1, 2, 4, 8, 16`.
    pub fn time_signature(mut self, num: i32, den: i32) -> Self {
        self.config.time_sig_numerator = num;
        self.config.time_sig_denominator = den;
        self
    }

    /// Enable or disable process isolation for plugin loading
    pub fn with_process_isolation(mut self, enabled: bool) -> Self {
        self.use_process_isolation = enabled;
        self
    }

    /// Add a custom plugin scan path
    pub fn add_scan_path<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.custom_paths.push(path.as_ref().to_path_buf());
        self
    }

    /// Enable scanning of default system VST3 paths
    pub fn scan_default_paths(mut self) -> Self {
        self.scan_default_paths = true;
        self
    }

    /// How long to wait for an isolated helper to respond before treating the plugin as hung
    /// (and killing the helper). Defaults to 5 seconds. Only affects process-isolated loads.
    pub fn response_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.response_timeout = Some(timeout);
        self
    }

    /// Override the path to the `vst3-host-helper` binary used for process isolation, instead
    /// of the default heuristic search. The `VST3_HOST_HELPER_PATH` environment variable does
    /// the same. Useful when the helper ships in a non-standard location.
    pub fn helper_path<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.helper_path = Some(path.into());
        self
    }

    /// Transparently respawn + reload a process-isolated plugin and retry the command when the
    /// helper crashes or hangs, instead of surfacing `Error::PluginCrashed`/`PluginTimeout` for
    /// the caller to handle via [`Plugin::recover`](crate::Plugin::recover).
    ///
    /// Only affects isolated loads and only the control plane — the audio-thread `process`
    /// path never recovers inline (a respawn would stall the callback). **Recovery reloads the
    /// plugin from defaults**: parameter values / state are NOT replayed, so snapshot with
    /// `save_state`/`load_state` if you need them preserved. Off by default.
    pub fn auto_recover_plugins(mut self, enabled: bool) -> Self {
        self.auto_recover_plugins = enabled;
        self
    }

    /// Max respawn+retry cycles per command when [`Self::auto_recover_plugins`] is on
    /// (default 1). `0` disables retries even if auto-recover is enabled.
    pub fn auto_recover_max_retries(mut self, retries: u32) -> Self {
        self.auto_recover_max_retries = Some(retries);
        self
    }

    /// Per-plugin timeout for the crash-resistant discovery probe used by
    /// [`Vst3Host::discover_plugins_safe`] (default
    /// [`DEFAULT_PROBE_TIMEOUT`](crate::discovery::DEFAULT_PROBE_TIMEOUT)). A plugin whose
    /// probe exceeds this is killed and skipped.
    pub fn probe_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.probe_timeout = Some(timeout);
        self
    }

    /// Build the configured host.
    ///
    /// Rejects a sample rate, block size or time signature the plugin setup can't honour, using
    /// the same rules as the corresponding runtime entry points
    /// ([`Plugin::reconfigure`](crate::Plugin::reconfigure),
    /// [`Plugin::set_time_signature`](crate::Plugin::set_time_signature)) — the configuration
    /// entry points previously disagreed, so a `block_size(0)` accepted here produced permanent
    /// silence, a `sample_rate(0.0)` reached `setupProcessing` where plugins computing
    /// `1.0 / sampleRate` generate NaN coefficients, and a `time_signature(4, 3)` built a host
    /// whose transport every runtime setter refuses.
    pub fn build(self) -> Result<Vst3Host> {
        if !self.config.sample_rate.is_finite() || self.config.sample_rate <= 0.0 {
            return Err(Error::InvalidParameter(format!(
                "sample rate must be finite and positive, got {}",
                self.config.sample_rate
            )));
        }
        if self.config.block_size == 0 || self.config.block_size > i32::MAX as usize {
            return Err(Error::InvalidParameter(format!(
                "block size must be in 1..={}, got {}",
                i32::MAX,
                self.config.block_size
            )));
        }
        if self.config.time_sig_numerator <= 0 {
            return Err(Error::InvalidParameter(format!(
                "time signature numerator must be positive, got {}",
                self.config.time_sig_numerator
            )));
        }
        if !matches!(self.config.time_sig_denominator, 1 | 2 | 4 | 8 | 16) {
            return Err(Error::InvalidParameter(format!(
                "time signature denominator must be one of 1, 2, 4, 8, 16, got {}",
                self.config.time_sig_denominator
            )));
        }
        Ok(Vst3Host {
            config: self.config,
            custom_paths: self.custom_paths,
            use_process_isolation: self.use_process_isolation,
            scan_default_paths: self.scan_default_paths,
            helper_path: self.helper_path,
            response_timeout: self
                .response_timeout
                .unwrap_or(crate::process_isolation::DEFAULT_RESPONSE_TIMEOUT),
            auto_recover_plugins: self.auto_recover_plugins,
            auto_recover_max_retries: self.auto_recover_max_retries.unwrap_or(1),
            probe_timeout: self
                .probe_timeout
                .unwrap_or(crate::discovery::DEFAULT_PROBE_TIMEOUT),
        })
    }
}

/// The outcome of [`Vst3Host::probe_plugin`] — whether a plugin can be loaded safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResult {
    /// The plugin loaded successfully in an isolated process.
    Ok,
    /// The plugin crashed the isolated helper while loading (do not load in-process).
    Crashed,
    /// The plugin did not respond within the timeout.
    TimedOut,
    /// Loading failed with an error (not a crash) — message included.
    Failed(String),
}

/// Plugin discovery progress information
#[derive(Debug, Clone)]
pub enum DiscoveryProgress {
    /// Discovery has started
    Started {
        /// Total number of plugins to scan
        total_plugins: usize,
    },
    /// A plugin was found
    Found {
        /// The plugin information
        plugin: PluginInfo,
        /// Current plugin index
        current: usize,
        /// Total number of plugins
        total: usize,
    },
    /// An error occurred while scanning a plugin
    Error {
        /// Path that failed
        path: String,
        /// Error message
        error: String,
    },
    /// Discovery completed
    Completed {
        /// Total number of plugins found
        total_found: usize,
    },
}

#[cfg(feature = "cpal-backend")]
impl Vst3Host {
    /// Load a plugin and immediately start playing it through the default audio
    /// output device, using the host's configured sample rate and block size.
    ///
    /// This is the "batteries-included" path: it wires a [`CpalBackend`] to the
    /// plugin and pumps audio for you. The returned [`AudioHandle`] keeps the stream
    /// alive — drop it to stop — and lets you keep sending MIDI / changing parameters
    /// while it plays:
    ///
    /// ```no_run
    /// # use vst3_host::Vst3Host;
    /// # use vst3_host::midi::MidiChannel;
    /// # fn main() -> vst3_host::Result<()> {
    /// let mut host = Vst3Host::new()?;
    /// let plugin = host.load_plugin("/path/to/synth.vst3")?;
    /// let audio = host.play(plugin)?;
    /// audio.lock().send_midi_note(60, 100, MidiChannel::Ch1)?;
    /// std::thread::sleep(std::time::Duration::from_secs(1));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// [`CpalBackend`]: crate::backends::CpalBackend
    /// [`AudioHandle`]: crate::AudioHandle
    pub fn play(&self, plugin: Plugin) -> Result<crate::AudioHandle> {
        let backend = crate::backends::CpalBackend::new()?;
        let config = crate::audio::AudioConfig {
            output_channels: 2,
            input_channels: 0,
            ..self.config
        };
        crate::playback::play_with_backend(&backend, plugin, config)
    }

    /// Host a plugin on **live audio input** (effect hosting): capture from the default input
    /// device, process through the plugin, and play the result on the default output device.
    ///
    /// Use this for effect plugins (EQ, reverb, compressor); for instruments use
    /// [`Self::play`]. Control the plugin via the returned [`AudioHandle`].
    ///
    /// [`AudioHandle`]: crate::AudioHandle
    pub fn play_with_input(&self, plugin: Plugin) -> Result<crate::AudioHandle> {
        let backend = crate::backends::CpalBackend::new()?;
        let config = crate::audio::AudioConfig {
            input_channels: 2,
            output_channels: 2,
            ..self.config
        };
        crate::playback::play_with_input_backend(&backend, plugin, config)
    }

    /// Play a plugin through the default device using the **lock-free** real-time path
    /// (a [`RealtimePluginRunner`]) instead of the mutex-based [`Self::play`].
    ///
    /// The audio callback takes no lock; queue MIDI and parameter changes through the
    /// returned handle's [`RtControl`](crate::RtControl):
    ///
    /// ```no_run
    /// # use vst3_host::{Vst3Host, midi::MidiEvent, midi::MidiChannel};
    /// # fn main() -> vst3_host::Result<()> {
    /// let mut host = Vst3Host::new()?;
    /// let plugin = host.load_plugin("/path/synth.vst3")?;
    /// let mut audio = host.play_realtime(plugin, 1024)?;
    /// audio.control().send_midi(MidiEvent::NoteOn { channel: MidiChannel::Ch1, note: 60, velocity: 100 });
    /// std::thread::sleep(std::time::Duration::from_secs(1));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// [`RealtimePluginRunner`]: crate::RealtimePluginRunner
    pub fn play_realtime(
        &self,
        plugin: Plugin,
        command_capacity: usize,
    ) -> Result<crate::playback::RtAudioHandle> {
        let backend = crate::backends::CpalBackend::new()?;
        let config = crate::audio::AudioConfig {
            output_channels: 2,
            input_channels: 0,
            ..self.config
        };
        crate::playback::play_realtime_with_backend(&backend, plugin, config, command_capacity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_defaults_to_120_bpm_4_4() {
        let host = Vst3HostBuilder::default().build().unwrap();
        assert_eq!(host.config().tempo, 120.0);
        assert_eq!(host.config().time_sig_numerator, 4);
        assert_eq!(host.config().time_sig_denominator, 4);
    }

    /// The builder used to accept any positive denominator, so a `4/3` host built fine and then
    /// had a transport that `Plugin::set_time_signature` and `RtControl` both refuse.
    #[test]
    fn builder_rejects_time_signatures_the_runtime_rejects() {
        for (num, den) in [(4, 3), (4, 0), (0, 4), (-1, 4), (4, 5), (4, 32), (4, -4)] {
            let built = Vst3HostBuilder::default().time_signature(num, den).build();
            assert!(
                built.is_err(),
                "builder accepted {num}/{den}, which every runtime setter rejects"
            );
        }
        // The denominators VST3 transports actually express still build.
        for den in [1, 2, 4, 8, 16] {
            assert!(Vst3HostBuilder::default()
                .time_signature(3, den)
                .build()
                .is_ok());
        }
    }

    /// The isolated load path sizes an `AudioLevels` allocation from a count the helper reports,
    /// so this boundary clamps it instead of trusting the peer.
    #[test]
    fn output_channel_count_from_a_peer_is_clamped() {
        assert_eq!(clamp_output_channels(2), 2);
        assert_eq!(clamp_output_channels(0), 2);
        assert_eq!(clamp_output_channels(-5), 2);
        assert_eq!(clamp_output_channels(MAX_OUTPUT_CHANNELS as i32), 256);
        assert_eq!(clamp_output_channels(i32::MAX), MAX_OUTPUT_CHANNELS);
    }

    #[test]
    fn builder_threads_tempo_and_time_signature_into_config() {
        let host = Vst3HostBuilder::default()
            .tempo(140.0)
            .time_signature(7, 8)
            .build()
            .unwrap();
        assert_eq!(host.config().tempo, 140.0);
        assert_eq!(host.config().time_sig_numerator, 7);
        assert_eq!(host.config().time_sig_denominator, 8);
    }
}
