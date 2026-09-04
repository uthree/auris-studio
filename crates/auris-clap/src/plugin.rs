//! The main-thread half of a hosted plugin.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Instant;

use auris_core::param::{ParamDescriptor, ParamId, ParamUnit};
use auris_core::plugin::{PluginDescriptor, PrepareContext};
use clack_extensions::audio_ports::PluginAudioPorts;
use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiError, PluginGui, Window};
use clack_extensions::latency::PluginLatency;
use clack_extensions::note_ports::{NotePortInfoBuffer, PluginNotePorts};
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_extensions::state::PluginState;
use clack_extensions::timer::PluginTimer;
use clack_host::prelude::*;
use clack_host::utils::ClapId;
use raw_window_handle::RawWindowHandle;

use crate::bridge::Bridge;
use crate::effect::ClapEffect;
use crate::error::ClapError;
use crate::host::{AurisHost, AurisMainThread, AurisShared, HostFlags, host_info};
use crate::instrument::ClapInstrument;
use crate::library::ClapPluginInfo;
use crate::notes::{NoteLanguage, language_for};
use crate::ports::PortLayout;
use crate::window::{ContainerWindow, HostWindow};

/// The plugin's parameters, in the order the plugin lists them.
///
/// Both halves of a hosted plugin need this and neither may change it, so it is shared by
/// [`Arc`] and never mutated. If the plugin ever asks for a rescan of anything but values, the
/// session must throw the whole plugin away and build it again — which is what
/// [`PendingRequests::restart`] is for.
pub(crate) struct ParamList {
    /// The plugin's own id for each parameter, indexed by [`ParamId`].
    pub(crate) clap_ids: Vec<ClapId>,
    /// What the rest of the application sees.
    pub(crate) descriptors: Vec<ParamDescriptor>,
}

/// Requests a plugin has made since this was last read.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingRequests {
    /// The plugin must be deactivated and rebuilt — its parameter list or layout changed.
    pub restart: bool,
    /// Parameter values changed inside the plugin; read them again.
    pub rescan_values: bool,
    /// The plugin's state changed, so the project is unsaved.
    pub dirty: bool,
    /// The plugin asked for its main-thread callback.
    pub callback: bool,
    /// The plugin's window is gone — the user closed it, or the plugin lost hold of it. The owner
    /// must call [`ClapPlugin::close_gui`] to finish the job, and stop telling anybody the editor
    /// is open.
    pub gui_closed: bool,
}

/// An instantiated CLAP plugin, living on the thread that made it.
///
/// This is the half that can answer questions. It is not [`Send`], deliberately: CLAP requires
/// that everything here happens on one thread, and the type system is the cheapest place to
/// enforce that. The rendering half is [`ClapEffect`], which this hands out on
/// [`activate`](Self::activate).
pub struct ClapPlugin {
    instance: PluginInstance<AurisHost>,
    info: ClapPluginInfo,
    params: Arc<ParamList>,
    active: bool,
    /// `true` between [`open_gui`](Self::open_gui) and [`close_gui`](Self::close_gui). The plugin
    /// is not asked, because CLAP gives no way to ask: `create` and `destroy` are a pair the host
    /// has to balance itself, and calling either twice is undefined.
    gui_open: bool,
    /// The window lent to a plugin that will only draw inside one of the host's.
    ///
    /// `None` for a plugin that makes its own, which is the arrangement to prefer and the minority
    /// of plugins. See [`crate::window`] for why the host has to be able to make one at all.
    container: Option<HostWindow>,
}

impl ClapPlugin {
    /// Instantiates a plugin from an already-loaded entry.
    pub(crate) fn new(entry: &PluginEntry, info: ClapPluginInfo) -> Result<Self, ClapError> {
        let id = std::ffi::CString::new(info.clap_id.as_str()).map_err(|_| {
            ClapError::UnknownPlugin(format!("{} (contains a NUL byte)", info.clap_id))
        })?;

        let mut instance = PluginInstance::<AurisHost>::new(
            |_| AurisShared::default(),
            |shared| AurisMainThread::new(shared),
            entry,
            &id,
            &host_info(),
        )
        .map_err(|error| ClapError::Instantiate {
            id: info.clap_id.clone(),
            reason: error.to_string(),
        })?;

        let params = Arc::new(read_params(&mut instance));

        Ok(Self {
            instance,
            info,
            params,
            active: false,
            gui_open: false,
            container: None,
        })
    }

    /// What the file said about this plugin.
    pub fn info(&self) -> &ClapPluginInfo {
        &self.info
    }

    /// Presents the plugin the way every other plugin in the application is presented.
    pub fn descriptor(&self) -> PluginDescriptor {
        self.info.descriptor()
    }

    /// The plugin's parameters.
    pub fn parameters(&self) -> &[ParamDescriptor] {
        &self.params.descriptors
    }

    /// Reads one parameter's current value from the plugin itself.
    ///
    /// The plugin is the authority here, not the host: a plugin's own interface, a preset it
    /// loaded, or its internal MIDI mapping can all move a parameter without the host asking.
    pub fn value(&mut self, id: ParamId) -> Option<f32> {
        let clap_id = *self.params.clap_ids.get(id.index())?;
        let params = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginParams>()?;
        params
            .get_value(&mut self.instance.plugin_handle(), clap_id)
            .map(|value| value as f32)
    }

    /// Requests the plugin has made since this was last called. Reading clears them.
    ///
    /// The window's close box is one of them, whichever window it was: a plugin that made its own
    /// reports through the host, and one drawing in a lent window has no idea it was closed —
    /// nobody told it, and nobody may until the host has taken its window apart in the right
    /// order.
    pub fn take_requests(&self) -> PendingRequests {
        let mut requests = self
            .instance
            .access_shared_handler(|shared| PendingRequests {
                restart: HostFlags::take(&shared.flags.restart)
                    | HostFlags::take(&shared.flags.rescan_info),
                rescan_values: HostFlags::take(&shared.flags.rescan_values),
                dirty: HostFlags::take(&shared.flags.dirty),
                callback: HostFlags::take(&shared.flags.callback),
                gui_closed: HostFlags::take(&shared.flags.gui_closed),
            });
        requests.gui_closed |= self
            .container
            .as_ref()
            .is_some_and(ContainerWindow::was_closed);
        requests
    }

    /// Runs the plugin's main-thread callback, which it asked for through
    /// [`PendingRequests::callback`].
    pub fn run_callback(&mut self) {
        self.instance.call_on_main_thread_callback();
    }

    /// Fires whichever of the plugin's timers are owed a tick.
    ///
    /// A CLAP plugin keeps no clock of its own: it asks the host for one and is called back here.
    /// Nothing else drives it, so a frontend that never calls this hosts plugins whose windows do
    /// not repaint. Call it from the same tick the interface repaints on — the timers are
    /// re-based on the present rather than caught up, so calling it late costs a late frame and
    /// not a flood of them.
    pub fn tick_timers(&mut self) {
        let due = self
            .instance
            .access_handler_mut(|main| main.due_timers(Instant::now()));
        if due.is_empty() {
            return;
        }
        let Some(timer) = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginTimer>()
        else {
            return;
        };
        for id in due {
            timer.on_timer(&mut self.instance.plugin_handle(), id);
        }
    }

    /// Whether this plugin has a window this host can put on screen.
    ///
    /// Two ways to answer no, and a caller can do nothing about either: the plugin draws nothing —
    /// plenty do not — or the only windowing it offers is something this platform has no API for.
    pub fn has_gui(&mut self) -> bool {
        self.gui().is_some()
    }

    /// How this plugin's window will be put up, or `None` if it has none.
    fn plan(&mut self) -> Option<GuiConfiguration<'static>> {
        let gui = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginGui>()?;
        let api_type = GuiApiType::default_for_current_platform()?;
        let offers = |floating: bool, plugin: &mut PluginInstance<AurisHost>| {
            gui.is_api_supported(
                &mut plugin.plugin_handle(),
                GuiConfiguration {
                    api_type,
                    is_floating: floating,
                },
            )
        };
        crate::gui::plan_for(
            offers(true, &mut self.instance),
            // A plugin that will only embed is only offered a window where there is a window to
            // lend it. Otherwise the answer is "it has one" and opening it fails, which is a
            // button on screen that does nothing.
            offers(false, &mut self.instance) && crate::window::CAN_LEND,
        )
    }

    /// Which windowing this plugin says it can do on this platform: floating, embedded, and what
    /// it would prefer.
    ///
    /// For working out why a plugin that plainly has a window reports none — which is a question
    /// only ever asked of a real plugin, and so is answered by
    /// `cargo run -p auris-clap --example inspect` rather than by anything in the application.
    pub fn window_apis(&mut self) -> Vec<String> {
        let Some(gui) = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginGui>()
        else {
            return vec!["no gui extension".to_string()];
        };
        let Some(api) = clack_extensions::gui::GuiApiType::default_for_current_platform() else {
            return vec!["no windowing api on this platform".to_string()];
        };

        let mut found = Vec::new();
        for floating in [true, false] {
            let plan = clack_extensions::gui::GuiConfiguration {
                api_type: api,
                is_floating: floating,
            };
            if gui.is_api_supported(&mut self.instance.plugin_handle(), plan) {
                found.push(match floating {
                    true => "floating".to_string(),
                    false => "embedded".to_string(),
                });
            }
        }
        if let Some(preferred) = gui.get_preferred_api(&mut self.instance.plugin_handle()) {
            found.push(format!(
                "prefers {:?}/{}",
                preferred.api_type,
                match preferred.is_floating {
                    true => "floating",
                    false => "embedded",
                }
            ));
        }
        found
    }

    /// `true` while the plugin's window is up.
    pub fn gui_is_open(&self) -> bool {
        self.gui_open
    }

    /// Opens the plugin's own window, above `parent`.
    ///
    /// Opening one that is already open does nothing, so a caller may treat this as "make sure it
    /// is showing" rather than having to track the state twice.
    ///
    /// Which of CLAP's two arrangements is used is the plugin's to decide and not the caller's: a
    /// plugin that makes its own window gets to, and one that will only draw inside somebody
    /// else's is lent a plain window of this crate's making. Either way what appears is a window
    /// above the application, and `parent` is what it stays above. Passing `None` is legal and
    /// gives a window that behaves like any other top-level one — which on Windows means sinking
    /// behind the application the moment the application is clicked.
    ///
    /// # Safety of the parent handle
    ///
    /// This is not an `unsafe` function, but it does hand out a window handle that is kept until
    /// [`close_gui`](Self::close_gui). Destroying that window first leaves a stale one behind.
    /// Nothing in Auris can reach that state: the window outlives the session, and dropping the
    /// session closes every plugin window first.
    pub fn open_gui(&mut self, parent: Option<RawWindowHandle>) -> Result<(), ClapError> {
        if self.gui_open {
            return Ok(());
        }
        let id = self.info.clap_id.clone();
        let no_gui = |reason: &str| ClapError::NoGui {
            id: id.clone(),
            reason: reason.to_string(),
        };
        let failed = |error: GuiError| ClapError::Gui {
            id: id.clone(),
            reason: error.to_string(),
        };

        let Some(plan) = self.plan() else {
            return Err(
                match self
                    .instance
                    .plugin_shared_handle()
                    .get_extension::<PluginGui>()
                    .is_some()
                {
                    true => no_gui("it draws no window this platform can show"),
                    false => no_gui("it implements no GUI at all"),
                },
            );
        };
        // PANIC: a plan is only answered after the extension has been read to answer it.
        let gui = self.gui().expect("a plan implies the extension");

        // Twice, because the two arrangements want it in two forms: a plugin managing its own
        // window is *suggested* a title through CLAP, which speaks C strings; a window this crate
        // makes is titled by the platform, which does not.
        let plain_title = crate::gui::window_title(&self.info.name);
        let title = crate::gui::suggested_title(&self.info.name);
        let mut handle = self.instance.plugin_handle();
        gui.create(&mut handle, plan).map_err(failed)?;
        // From here every exit owes the plugin a `destroy`. Failures pay it below rather than
        // recording `gui_open` early: an early flag over no window would satisfy the opening
        // early-return, and every later `open_gui` would answer `Ok` while doing nothing —
        // with `close_gui`, and the create/destroy balance, waiting on a window that never
        // existed.
        // Kept outside the fallible setup closure so an embedded container remains alive while
        // the error arm calls `destroy`. On Win32, dropping it first recursively destroys the
        // plugin's newly parented child HWND.
        let mut pending_container = None;
        let outcome = (|| {
            if plan.is_floating {
                if let Some(parent) = parent.and_then(Window::from_window_handle) {
                    // SAFETY: the handle came from a live window — see this function's own note
                    // on what keeps it live. A plugin that will not take a transient parent
                    // still gets a window, one that can fall behind the application.
                    if let Err(error) = unsafe { gui.set_transient(&mut handle, parent) } {
                        log::debug!("`{id}` refused a parent window: {error}");
                    }
                }
                gui.suggest_title(&mut handle, &title);
                gui.show(&mut handle).map_err(failed)?;
                return Ok(None);
            }

            // Embedded, so the window is this crate's to make. The order is CLAP's and every
            // step of it depends on the one before: the window has to exist before its scale
            // can be read, because the scale belongs to the display it landed on, and the scale
            // has to be set before the size is asked for, or the plugin answers for a display
            // it was never told about.
            pending_container = Some(
                HostWindow::open(&plain_title, crate::window::PROVISIONAL, parent)
                    .ok_or_else(|| no_gui("this platform would not lend the plugin a window"))?,
            );
            let container = pending_container
                .as_ref()
                .expect("the container was assigned above");
            if container.scale() != 1.0 {
                let _ = gui.set_scale(&mut handle, container.scale());
            }
            if let Some(size) = gui.get_size(&mut handle) {
                container.resize(size);
            }
            let Some(window) = Window::from_window_handle(container.handle()) else {
                return Err(no_gui(
                    "this platform's window is not one CLAP can be given",
                ));
            };

            // SAFETY: the window was made a few lines up and is owned by this value, which
            // destroys the plugin's GUI before letting go of it.
            unsafe { gui.set_parent(&mut handle, window) }.map_err(failed)?;
            gui.show(&mut handle).map_err(failed)?;
            // Last: an empty window that fills a moment later reads as a flicker, and this way
            // the plugin has already drawn into it by the time anybody sees it.
            container.show();
            Ok(pending_container.take())
        })();

        match outcome {
            Ok(container) => {
                self.gui_open = true;
                self.container = container;
                Ok(())
            }
            Err(error) => {
                // For an embedded GUI, `pending_container` still owns the parent window here.
                // CLAP requires that window to remain valid until `destroy` returns.
                gui.destroy(&mut handle);
                // The supported platform implementations release a native window in `Drop`.
                // On unsupported platforms the uninhabited placeholder has no `Drop` impl,
                // which makes this intentionally ordered destruction look redundant to Clippy.
                #[allow(clippy::drop_non_drop)]
                drop(pending_container);
                Err(error)
            }
        }
    }

    /// Closes the plugin's window and frees what it was drawn with.
    ///
    /// Closing one that is not open does nothing. This is also what answers a plugin that reported
    /// its window gone through [`PendingRequests::gui_closed`] — CLAP requires the host to call
    /// `destroy` to acknowledge that, and skipping it leaks the plugin's own resources.
    pub fn close_gui(&mut self) {
        if !self.gui_open {
            return;
        }
        self.gui_open = false;
        let Some(gui) = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginGui>()
        else {
            return;
        };
        // Whether the window still exists. A plugin that destroyed its own window and told the
        // host so is owed `destroy` and nothing else; hiding it first would be a call through a
        // pointer it has already freed.
        let destroyed = self
            .instance
            .access_shared_handler(|shared| HostFlags::take(&shared.flags.gui_destroyed));

        let mut handle = self.instance.plugin_handle();
        if !destroyed {
            let _ = gui.hide(&mut handle);
        }
        gui.destroy(&mut handle);
        // Only now: the plugin's own window is a child of this one, and taking the ground away
        // before `destroy` has been answered is the crash the container's close box goes out of
        // its way not to cause either.
        self.container = None;
    }

    /// Stands in for the plugin reporting that its window has gone.
    ///
    /// The real call arrives from inside the plugin, at the moment somebody clicks the window's
    /// close box — which a test runner has no window to click. Everything downstream of the flag
    /// is the same either way, and the flag itself is set by the same function the callback calls.
    ///
    /// Behind the same feature as [`testkit`](crate::testkit), and for the same reason: the crates
    /// above this one have to be able to test what they do about it.
    #[cfg(any(test, feature = "testkit"))]
    pub fn pretend_the_window_closed(&mut self, destroyed: bool) {
        use clack_extensions::gui::HostGuiImpl;
        self.instance
            .access_shared_handler(|shared| shared.closed(destroyed));
    }

    /// Stands in for the plugin reporting that it has changed something about itself.
    ///
    /// The real call arrives when somebody loads a preset in the plugin's own window or turns a
    /// knob there, which a fixture has no window to do. As above, what is under test is everything
    /// downstream of the flag, and the flag is set by the same function the callback calls.
    #[cfg(any(test, feature = "testkit"))]
    pub fn pretend_the_state_changed(&mut self) {
        use clack_extensions::state::HostStateImpl;
        self.instance.access_handler_mut(|main| main.mark_dirty());
    }

    /// The GUI extension, but only when it can give a window on this platform.
    fn gui(&mut self) -> Option<PluginGui> {
        self.plan()?;
        self.instance
            .plugin_shared_handle()
            .get_extension::<PluginGui>()
    }

    /// Activates the plugin and hands out the half that renders.
    ///
    /// A CLAP plugin allocates its buffers here, from the rate and block size it is given, and
    /// must be deactivated and activated again if either changes. That is why the effect's
    /// [`prepare`](auris_core::plugin::Effect::prepare) does nothing: by the time the graph
    /// exists, preparing has already happened, and the only honest response to a rate change is
    /// to build the plugin again.
    pub fn activate(&mut self, ctx: &PrepareContext) -> Result<ClapEffect, ClapError> {
        Ok(ClapEffect::new(self.start(ctx)?))
    }

    /// Activates the plugin as a track's sound source.
    ///
    /// The same activation as [`activate`](Self::activate) — CLAP has one kind of plugin and two
    /// habits — with the note port read as well, so the instrument knows what language to send
    /// notes in. A plugin with no note input gets none, and plays nothing; that is a plugin being
    /// used as an instrument when it is an effect, and the silence says so.
    pub fn activate_instrument(
        &mut self,
        ctx: &PrepareContext,
    ) -> Result<ClapInstrument, ClapError> {
        Ok(ClapInstrument::new(self.start(ctx)?))
    }

    /// Activates the plugin and wraps the processor with everything the audio thread will need.
    fn start(&mut self, ctx: &PrepareContext) -> Result<Bridge, ClapError> {
        let config = PluginAudioConfiguration {
            sample_rate: ctx.sample_rate,
            min_frames_count: 1,
            max_frames_count: ctx.max_block_frames.max(1) as u32,
        };

        // Read before activating: the ports and the note port may only change while the plugin is
        // deactivated, so asking now is asking at the last moment they can still move.
        let ports = self.ports(ctx.channel_count.max(1));
        let language = self.note_language();

        let processor =
            self.instance
                .activate(|_, _| (), config)
                .map_err(|error| ClapError::Activate {
                    id: self.info.clap_id.clone(),
                    sample_rate: ctx.sample_rate,
                    max_block_frames: ctx.max_block_frames,
                    reason: error.to_string(),
                })?;
        self.active = true;

        let latency = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginLatency>()
            .map(|latency| latency.get(&mut self.instance.plugin_handle()) as usize)
            .unwrap_or(0);

        Ok(Bridge::new(
            processor,
            self.descriptor(),
            Arc::clone(&self.params),
            ports,
            language,
            ctx,
            latency,
        ))
    }

    /// What the plugin's first note input port speaks, or `None` if it has none.
    ///
    /// Only the first port is consulted. A plugin with several is describing more streams than a
    /// track has to send it, and picking one of them arbitrarily would be a guess; the first is
    /// the one CLAP calls the main one.
    pub fn note_language(&mut self) -> Option<NoteLanguage> {
        let ports = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginNotePorts>()?;
        let mut handle = self.instance.plugin_handle();
        // Count before get, the rule `ports` and `read_params` already keep: the index means
        // "into the array count() declared", and a plugin with note *outputs* only — count of
        // zero inputs — handed an index 0 anyway is the exact off-the-end read that `ports`'s
        // module doc watched take an application down.
        if ports.count(&mut handle, true) == 0 {
            return None;
        }
        let mut buffer = NotePortInfoBuffer::new();
        let info = ports.get(&mut handle, 0, true, &mut buffer)?;
        language_for(info.supported_dialects)
    }

    /// The audio ports the plugin declares, or a stereo pair if it declares none.
    ///
    /// Read afresh each time it is asked for rather than cached, because a plugin is allowed to
    /// change its ports while deactivated — and the one moment this is asked is the one moment
    /// after which it may not.
    pub fn ports(&mut self, fallback_channels: usize) -> PortLayout {
        let Some(ports) = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginAudioPorts>()
        else {
            return PortLayout::stereo_pair(fallback_channels);
        };

        let mut handle = self.instance.plugin_handle();
        let (inputs, main_input) = crate::ports::read_side(&ports, &mut handle, true);
        let (outputs, main_output) = crate::ports::read_side(&ports, &mut handle, false);
        PortLayout {
            inputs,
            outputs,
            main_input,
            main_output,
        }
    }

    /// Deactivates the plugin, taking back the rendering half.
    pub fn deactivate(&mut self, effect: ClapEffect) {
        self.instance.deactivate(effect.into_stopped());
        self.active = false;
    }

    /// Deactivates the plugin, taking back the instrument half.
    pub fn deactivate_instrument(&mut self, instrument: ClapInstrument) {
        self.instance.deactivate(instrument.into_stopped());
        self.active = false;
    }

    /// Deactivates a plugin whose rendering half was dropped somewhere else, and reports whether
    /// it now has no effect out.
    ///
    /// This is the normal path in Auris Studio: a replaced graph travels back from the audio
    /// thread down the engine's return channel and is dropped there, so by the time the session
    /// gets round to the plugin the effect is already gone. Returns `false` when the effect is
    /// still alive, in which case the caller should try again later rather than force it.
    ///
    /// The answer is about the *state*, not about this call, so asking twice gives the same
    /// answer both times. It used to mean "was deactivated by this call", which made a second
    /// ask say `false` about a plugin that was perfectly free — and a caller keeping a spare
    /// would throw it away on the strength of that.
    pub fn release(&mut self) -> bool {
        if !self.active {
            return true;
        }
        self.active = self.instance.try_deactivate().is_err();
        !self.active
    }

    /// `true` while a rendering half exists.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The plugin's own state, as the opaque byte stream CLAP defines it to be.
    ///
    /// There is no way around the opacity: a CLAP plugin's state is whatever it says it is, and
    /// a wavetable or a sample path cannot be squeezed into the `f32` map that
    /// [`auris_core::plugin::PluginState`] carries. The session stores these bytes beside that
    /// map rather than in it.
    pub fn save_state(&mut self) -> Result<Vec<u8>, ClapError> {
        let state = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginState>()
            .ok_or_else(|| ClapError::State {
                id: self.info.clap_id.clone(),
                saving: true,
                reason: "the plugin implements no state extension".into(),
            })?;

        let mut bytes = Vec::new();
        state
            .save(&mut self.instance.plugin_handle(), &mut bytes)
            .map_err(|error| ClapError::State {
                id: self.info.clap_id.clone(),
                saving: true,
                reason: error.to_string(),
            })?;
        Ok(bytes)
    }

    /// Restores state previously taken by [`save_state`](Self::save_state).
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), ClapError> {
        let state = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginState>()
            .ok_or_else(|| ClapError::State {
                id: self.info.clap_id.clone(),
                saving: false,
                reason: "the plugin implements no state extension".into(),
            })?;

        state
            .load(&mut self.instance.plugin_handle(), &mut &bytes[..])
            .map_err(|error| ClapError::State {
                id: self.info.clap_id.clone(),
                saving: false,
                reason: error.to_string(),
            })
    }
}

impl Drop for ClapPlugin {
    /// Closes the window before the plugin behind it goes away.
    ///
    /// CLAP requires `destroy` before the instance is destroyed, and the instance is destroyed by
    /// the field below this one. Leaving it to the plugin's own destructor is not a thing a host
    /// may do: a window is an OS object registered against this process, and the code that would
    /// clean it up is inside the library that is about to be unmapped.
    ///
    /// This is why a slot may drop a plugin whose editor is open without thinking about it — which
    /// it does, every time a track with an open editor is deleted.
    fn drop(&mut self) {
        self.close_gui();
    }
}

impl std::fmt::Debug for ClapPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClapPlugin")
            .field("id", &self.info.clap_id)
            .field("parameters", &self.params.descriptors.len())
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

/// Most parameter descriptions accepted from one plugin.
///
/// Large synths can legitimately expose thousands; a count near `u32::MAX` can only be corrupt
/// and must not become an allocator request.
const MAX_PARAMS: u32 = 4_096;

/// Largest stepped range represented as a picker rather than a stepped plain control.
const MAX_PARAM_CHOICES: u32 = 1_024;

fn bounded_param_count(count: u32) -> u32 {
    count.min(MAX_PARAMS)
}

/// Asks a freshly instantiated plugin what its parameters are.
fn read_params(instance: &mut PluginInstance<AurisHost>) -> ParamList {
    let Some(params) = instance
        .plugin_shared_handle()
        .get_extension::<PluginParams>()
    else {
        return ParamList {
            clap_ids: Vec::new(),
            descriptors: Vec::new(),
        };
    };

    let count = bounded_param_count(params.count(&mut instance.plugin_handle()));
    let mut clap_ids = Vec::with_capacity(count as usize);
    let mut descriptors = Vec::with_capacity(count as usize);
    let mut buffer = ParamInfoBuffer::new();

    for index in 0..count {
        let mut handle = instance.plugin_handle();
        let Some(info) = params.get_info(&mut handle, index, &mut buffer) else {
            continue;
        };
        // The position in *our* slice, not the plugin's index: a parameter the plugin refused
        // to describe is skipped, and everything after it shifts up.
        let id = descriptors.len() as u32;
        descriptors.push(describe(
            id,
            info.id.get(),
            &String::from_utf8_lossy(info.name),
            info.min_value,
            info.max_value,
            info.default_value,
            info.flags.contains(ParamInfoFlags::IS_STEPPED),
        ));
        clap_ids.push(info.id);
    }

    ParamList {
        clap_ids,
        descriptors,
    }
}

/// Turns one CLAP parameter description into the one the rest of the application speaks.
///
/// The two mismatches worth naming:
///
/// * CLAP ids are arbitrary `u32`s chosen by the plugin, while a [`ParamId`] is a position in a
///   slice. So the position is assigned here and the plugin's id is kept alongside for talking
///   back to it.
/// * A saved project keys parameters by string. The plugin's own id is the only thing about a
///   CLAP parameter that is promised never to change — the *name* may change, the *index* may
///   move — so the key is built from it, and a project keeps loading after the plugin is updated
///   and reorders its list.
/// * The range is the plugin's arithmetic, not ours. `ParamDescriptor` refuses to die of a range
///   that is inverted or not a number, but a value quietly landing somewhere other than where the
///   plugin meant it is still wrong — and this is the last place the plugin can be *named*, so the
///   range is corrected and reported here, once, rather than puzzled over later from a knob that
///   will not move.
fn describe(
    id: u32,
    clap_id: u32,
    name: &str,
    min: f64,
    max: f64,
    default: f64,
    stepped: bool,
) -> ParamDescriptor {
    let (min, max) = match (min as f32, max as f32) {
        (min, max) if min.is_finite() && max.is_finite() && min <= max => (min, max),
        (min, max) => {
            log::warn!("parameter {clap_id} reports the range {min}..{max}; reading it as 0..1");
            (0.0, 1.0)
        }
    };
    let default = match default as f32 {
        default if default.is_finite() => default.clamp(min, max),
        _ => min,
    };
    let steps = match stepped {
        // A stepped CLAP parameter is a range of integers, ends included.
        true => Some(((max - min).round() as u32).saturating_add(1)),
        false => None,
    };
    let mut unit = match (stepped, min, max) {
        (true, 0.0, 1.0) => ParamUnit::Toggle,
        (true, _, _) => ParamUnit::Choice,
        _ => ParamUnit::Plain,
    };
    let choices: Cow<'static, [Cow<'static, str>]> = match (unit, steps) {
        (ParamUnit::Choice, Some(count)) if count <= MAX_PARAM_CHOICES => Cow::Owned(
            (0..count)
                .map(|index| {
                    let value = min + index as f32;
                    Cow::Owned(if value.fract() == 0.0 {
                        format!("{value:.0}")
                    } else {
                        value.to_string()
                    })
                })
                .collect(),
        ),
        (ParamUnit::Choice, _) => {
            // A menu with millions of rows is neither usable nor safe to allocate. It remains
            // stepped, so the ordinary control still lands on the plugin's discrete values.
            unit = ParamUnit::Plain;
            Cow::Borrowed(&[])
        }
        _ => Cow::Borrowed(&[]),
    };

    ParamDescriptor {
        id: ParamId(id),
        key: Cow::Owned(format!("clap.{clap_id}")),
        name: Cow::Owned(match name.is_empty() {
            true => format!("Parameter {clap_id}"),
            false => name.to_string(),
        }),
        min,
        max,
        default,
        unit,
        curve: auris_core::param::ParamValueCurve::Linear,
        steps,
        choices,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parameter_is_keyed_by_the_plugins_own_id_not_its_position() {
        // The plugin may reorder its list in the next version; it may not change an id. Keying
        // by position would silently move a saved value onto a different parameter.
        let first = describe(0, 4242, "Gain", 0.0, 2.0, 1.0, false);
        let moved = describe(3, 4242, "Gain", 0.0, 2.0, 1.0, false);
        assert_eq!(first.key, moved.key);
        assert_eq!(first.key, "clap.4242");
        assert_eq!(first.id, ParamId(0));
        assert_eq!(moved.id, ParamId(3));
    }

    #[test]
    fn a_nameless_parameter_still_gets_a_label() {
        let param = describe(0, 7, "", 0.0, 1.0, 0.5, false);
        assert_eq!(param.name, "Parameter 7");
    }

    #[test]
    fn a_two_position_stepped_parameter_is_a_toggle() {
        let toggle = describe(0, 1, "Bypass", 0.0, 1.0, 0.0, true);
        assert_eq!(toggle.unit, ParamUnit::Toggle);
        assert_eq!(toggle.steps, Some(2));

        let choice = describe(0, 2, "Mode", 0.0, 3.0, 0.0, true);
        assert_eq!(choice.unit, ParamUnit::Choice);
        assert_eq!(choice.steps, Some(4), "four positions, both ends included");
        assert_eq!(
            choice.choices.as_ref(),
            ["0", "1", "2", "3"],
            "a choice always has one selectable label per position"
        );

        let continuous = describe(0, 3, "Drive", 0.0, 1.0, 0.5, false);
        assert_eq!(continuous.unit, ParamUnit::Plain);
        assert_eq!(continuous.steps, None);
    }

    #[test]
    fn absurd_parameter_counts_and_choice_ranges_do_not_drive_allocations() {
        let reported = std::hint::black_box(u32::MAX);
        assert_eq!(bounded_param_count(reported), MAX_PARAMS);
        let enormous = describe(0, 4, "Unsafe", 0.0, u32::MAX as f64, 0.0, true);
        assert_eq!(enormous.unit, ParamUnit::Plain);
        assert!(enormous.choices.is_empty());
        assert_eq!(enormous.steps, Some(u32::MAX));
    }

    #[test]
    fn a_range_the_plugin_reported_backwards_is_corrected_here() {
        // Not merely survived: a descriptor carrying an inverted range would have every value
        // silently clamped against the unit range instead, which is not what the plugin's own
        // numbers say either. So the correction happens once, where the plugin can be named.
        let backwards = describe(0, 9, "Cutoff", 20_000.0, 20.0, 1_000.0, false);
        assert_eq!(backwards.min, 0.0);
        assert_eq!(backwards.max, 1.0);
        assert_eq!(
            backwards.default, 1.0,
            "a default outside the range it lands in is clamped, not carried through"
        );

        let nan = describe(0, 10, "Broken", f64::NAN, 1.0, 0.5, false);
        assert_eq!((nan.min, nan.max), (0.0, 1.0));

        let nan_default = describe(0, 11, "Broken", -6.0, 6.0, f64::NAN, false);
        assert_eq!(
            nan_default.default, -6.0,
            "a not-a-number default is the bottom of the range, never a not-a-number"
        );
    }

    #[test]
    fn a_range_the_plugin_reported_properly_is_left_exactly_alone() {
        let param = describe(0, 12, "Cutoff", 20.0, 20_000.0, 440.0, false);
        assert_eq!(
            (param.min, param.max, param.default),
            (20.0, 20_000.0, 440.0)
        );
    }
}
