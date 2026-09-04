//! `auris.fx.eq` — a six band parametric equaliser.

use auris_core::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId, ParamValueCurve};
use auris_core::plugin::{
    Effect, Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};

use crate::bank::ParamBank;
use crate::biquad::{Biquad, BiquadCoefficients};

/// Number of bands in the equaliser.
pub const BAND_COUNT: usize = 6;

/// The plugin id, so a display can recognise the one effect that has a curve to draw.
///
/// A constant rather than a string spelled again in the frontend: an editor that draws an
/// equalizer's response has to know which plugin it is looking at, and a typo in that string is
/// an editor that silently stops drawing.
pub const ID: &str = "auris.fx.eq";

/// Parameters exposed per band.
const PARAMS_PER_BAND: u32 = 4;

/// Offsets of the four parameters within a band.
const OFFSET_ENABLED: u32 = 0;
const OFFSET_FREQUENCY: u32 = 1;
const OFFSET_GAIN: u32 = 2;
const OFFSET_Q: u32 = 3;

/// The filter shape of one band.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EqBandKind {
    /// Two-pole high-pass. Ignores the band's gain.
    HighPass,
    /// Low shelf.
    LowShelf,
    /// Bell.
    Peaking,
    /// High shelf.
    HighShelf,
    /// Two-pole low-pass. Ignores the band's gain.
    LowPass,
}

impl EqBandKind {
    /// Whether the band's gain parameter does anything.
    ///
    /// A pass filter has a corner, not a level, and `design` never hands its gain to the
    /// cookbook. A display needs to know: a node dragged up and down on a high-pass would move
    /// a number nothing reads, which looks exactly like a control that is broken.
    pub const fn has_gain(self) -> bool {
        !matches!(self, EqBandKind::HighPass | EqBandKind::LowPass)
    }

    fn design(self, sample_rate: f64, frequency: f32, q: f32, gain_db: f32) -> BiquadCoefficients {
        match self {
            EqBandKind::HighPass => BiquadCoefficients::highpass(sample_rate, frequency, q),
            EqBandKind::LowShelf => {
                BiquadCoefficients::low_shelf(sample_rate, frequency, q, gain_db)
            }
            EqBandKind::Peaking => BiquadCoefficients::peaking(sample_rate, frequency, q, gain_db),
            EqBandKind::HighShelf => {
                BiquadCoefficients::high_shelf(sample_rate, frequency, q, gain_db)
            }
            EqBandKind::LowPass => BiquadCoefficients::lowpass(sample_rate, frequency, q),
        }
    }
}

/// Everything that distinguishes one band from another at construction time.
#[derive(Copy, Clone)]
struct BandSpec {
    kind: EqBandKind,
    label: &'static str,
    keys: [&'static str; 4],
    names: [&'static str; 4],
    frequency: f32,
    q: f32,
    enabled: bool,
}

/// The fixed band layout: pass filters at the edges, shelves inside them, bells in the middle.
///
/// Keys are stable strings written into project files, so they must never change. The two pass
/// bands start disabled — an insert with default settings is then a bypass — while the four
/// gain bands start enabled at 0 dB, which the cookbook designs make exactly transparent.
const BANDS: [BandSpec; BAND_COUNT] = [
    BandSpec {
        kind: EqBandKind::HighPass,
        label: "High Pass",
        keys: ["hp_enabled", "hp_freq", "hp_gain", "hp_q"],
        names: ["HP On", "HP Freq", "HP Gain", "HP Q"],
        frequency: 30.0,
        q: 0.707,
        enabled: false,
    },
    BandSpec {
        kind: EqBandKind::LowShelf,
        label: "Low Shelf",
        keys: ["ls_enabled", "ls_freq", "ls_gain", "ls_q"],
        names: ["LS On", "LS Freq", "LS Gain", "LS Q"],
        frequency: 120.0,
        q: 0.707,
        enabled: true,
    },
    BandSpec {
        kind: EqBandKind::Peaking,
        label: "Peak 1",
        keys: ["p1_enabled", "p1_freq", "p1_gain", "p1_q"],
        names: ["P1 On", "P1 Freq", "P1 Gain", "P1 Q"],
        frequency: 600.0,
        q: 1.0,
        enabled: true,
    },
    BandSpec {
        kind: EqBandKind::Peaking,
        label: "Peak 2",
        keys: ["p2_enabled", "p2_freq", "p2_gain", "p2_q"],
        names: ["P2 On", "P2 Freq", "P2 Gain", "P2 Q"],
        frequency: 3_000.0,
        q: 1.0,
        enabled: true,
    },
    BandSpec {
        kind: EqBandKind::HighShelf,
        label: "High Shelf",
        keys: ["hs_enabled", "hs_freq", "hs_gain", "hs_q"],
        names: ["HS On", "HS Freq", "HS Gain", "HS Q"],
        frequency: 8_000.0,
        q: 0.707,
        enabled: true,
    },
    BandSpec {
        kind: EqBandKind::LowPass,
        label: "Low Pass",
        keys: ["lp_enabled", "lp_freq", "lp_gain", "lp_q"],
        names: ["LP On", "LP Freq", "LP Gain", "LP Q"],
        frequency: 18_000.0,
        q: 0.707,
        enabled: false,
    },
];

/// Where one band's parameters are, and what shape they make.
///
/// What an editor needs in order to draw a band without knowing anything else about the plugin:
/// which four keys carry it, what it is called, and which of the five shapes it is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct EqBandLayout {
    /// The filter shape.
    pub kind: EqBandKind,
    /// What the band is called.
    pub label: &'static str,
    /// The keys of its four parameters, in the order on, frequency, gain, Q.
    pub keys: [&'static str; 4],
}

/// The bands an [`Equalizer`] has, from the lowest to the highest.
///
/// Read off the same table the plugin designs its filters from, so a curve on screen and the
/// audio it stands for cannot come apart: a band added to `BANDS` appears on the display without
/// anything in a frontend being told about it.
pub const LAYOUT: [EqBandLayout; BAND_COUNT] = layout();

const fn layout() -> [EqBandLayout; BAND_COUNT] {
    let mut out = [EqBandLayout {
        kind: EqBandKind::Peaking,
        label: "",
        keys: [""; 4],
    }; BAND_COUNT];
    let mut band = 0;
    while band < BAND_COUNT {
        out[band] = EqBandLayout {
            kind: BANDS[band].kind,
            label: BANDS[band].label,
            keys: BANDS[band].keys,
        };
        band += 1;
    }
    out
}

/// What one band is set to, as something outside the plugin reads it out of a document.
///
/// The plugin holds these as parameters and turns them into coefficients once; an editor holds
/// them as numbers it is about to draw and hand back. [`response_db`] is the arithmetic both
/// need, and it lives here for the same reason [`LAYOUT`] does.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EqBandSetting {
    /// The filter shape.
    pub kind: EqBandKind,
    /// Whether the band is switched in. One that is not contributes nothing to the curve.
    pub enabled: bool,
    /// Centre or corner frequency, in hertz.
    pub frequency: f32,
    /// Gain in dB, which the shapes [`EqBandKind::has_gain`] denies do not read.
    pub gain_db: f32,
    /// Resonance.
    pub q: f32,
}

/// The combined response of `bands` at `frequency_hz`, in dB.
///
/// The same sum [`Equalizer::magnitude_db`] makes, from settings rather than from a running
/// plugin — the bands are in series, so their responses multiply, which is an addition once
/// expressed in dB.
pub fn response_db(bands: &[EqBandSetting], sample_rate: f64, frequency_hz: f32) -> f32 {
    bands
        .iter()
        .filter(|band| band.enabled)
        .map(|band| {
            band.kind
                .design(sample_rate, band.frequency, band.q, band.gain_db)
                .magnitude_db(frequency_hz, sample_rate)
        })
        .sum()
}

/// Lowest frequency a band may be set to, in hertz.
pub const MIN_FREQUENCY: f32 = 20.0;
/// Highest frequency a band may be set to, in hertz.
pub const MAX_FREQUENCY: f32 = 20_000.0;
/// How far a band's gain reaches either side of flat, in dB.
pub const MAX_GAIN_DB: f32 = 24.0;
/// Least resonant a band may be set.
pub const MIN_Q: f32 = 0.1;
/// Most resonant a band may be set.
pub const MAX_Q: f32 = 18.0;

/// A serial chain of six independently switchable filter bands.
///
/// Each band owns one [`Biquad`] per channel, so the channels never share filter memory.
pub struct Equalizer {
    params: ParamBank,
    coefficients: [BiquadCoefficients; BAND_COUNT],
    enabled: [bool; BAND_COUNT],
    /// One filter set per channel, sized by [`Effect::prepare`].
    filters: Vec<[Biquad; BAND_COUNT]>,
    sample_rate: f64,
}

impl Default for Equalizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Equalizer {
    /// A new equaliser with every band flat.
    pub fn new() -> Self {
        let mut descriptors = Vec::with_capacity(BAND_COUNT * PARAMS_PER_BAND as usize);
        for (index, spec) in BANDS.iter().enumerate() {
            let base = index as u32 * PARAMS_PER_BAND;
            descriptors.push(ParamDescriptor::toggle(
                base + OFFSET_ENABLED,
                spec.keys[0],
                spec.names[0],
                spec.enabled,
            ));
            descriptors.push(ParamDescriptor::hertz(
                base + OFFSET_FREQUENCY,
                spec.keys[1],
                spec.names[1],
                MIN_FREQUENCY,
                MAX_FREQUENCY,
                spec.frequency,
            ));
            descriptors.push(ParamDescriptor::decibels(
                base + OFFSET_GAIN,
                spec.keys[2],
                spec.names[2],
                -MAX_GAIN_DB,
                MAX_GAIN_DB,
                0.0,
            ));
            descriptors.push(
                ParamDescriptor::new(
                    base + OFFSET_Q,
                    spec.keys[3],
                    spec.names[3],
                    MIN_Q,
                    MAX_Q,
                    spec.q,
                )
                .with_curve(ParamValueCurve::Logarithmic),
            );
        }

        let mut plugin = Self {
            params: ParamBank::new(descriptors),
            coefficients: [BiquadCoefficients::identity(); BAND_COUNT],
            enabled: [false; BAND_COUNT],
            filters: Vec::new(),
            sample_rate: 48_000.0,
        };
        plugin.recompute_all();
        plugin
    }

    /// Human-readable name of a band, for the UI.
    pub fn band_label(band: usize) -> &'static str {
        BANDS.get(band).map_or("", |spec| spec.label)
    }

    /// The filter shape of a band.
    pub fn band_kind(band: usize) -> Option<EqBandKind> {
        BANDS.get(band).map(|spec| spec.kind)
    }

    /// Coefficients currently in use by a band, whether or not it is enabled.
    pub fn band_coefficients(&self, band: usize) -> Option<BiquadCoefficients> {
        self.coefficients.get(band).copied()
    }

    /// Combined magnitude response of the enabled bands at `frequency_hz`, in dB.
    ///
    /// The bands run in series, so their individual responses multiply — which is a sum once
    /// expressed in dB. This is what the UI plots; no audio has to be pushed through the chain.
    pub fn magnitude_db(&self, frequency_hz: f32) -> f32 {
        let mut total = 0.0;
        for (band, coefficients) in self.coefficients.iter().enumerate() {
            if self.enabled[band] {
                total += coefficients.magnitude_db(frequency_hz, self.sample_rate);
            }
        }
        total
    }

    fn recompute_band(&mut self, band: usize) {
        let Some(spec) = BANDS.get(band) else {
            return;
        };
        let base = band as u32 * PARAMS_PER_BAND;
        let enabled = self.params.flag(base + OFFSET_ENABLED);
        if enabled != self.enabled[band] {
            // A bypassed recursive filter does not consume samples, so its delay state would
            // otherwise freeze and burst back out when the band is enabled again.
            for channel in &mut self.filters {
                channel[band].reset();
            }
        }
        self.enabled[band] = enabled;
        self.coefficients[band] = spec.kind.design(
            self.sample_rate,
            self.params.at(base + OFFSET_FREQUENCY),
            self.params.at(base + OFFSET_Q),
            self.params.at(base + OFFSET_GAIN),
        );
    }

    fn recompute_all(&mut self) {
        for band in 0..BAND_COUNT {
            self.recompute_band(band);
        }
    }
}

impl Parameterized for Equalizer {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.params.descriptors()
    }

    fn param(&self, id: ParamId) -> f32 {
        self.params.get(id)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        if self.params.set(id, value) {
            // Only the touched band's coefficients can have changed.
            self.recompute_band(id.index() / PARAMS_PER_BAND as usize);
        }
    }
}

impl Effect for Equalizer {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            ID,
            "Equalizer",
            "Six band EQ: high-pass, low shelf, two bells, high shelf, low-pass",
            PluginCategory::Equalizer,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate;
        self.filters
            .resize_with(ctx.channel_count.max(1), || [Biquad::default(); BAND_COUNT]);
        self.recompute_all();
        self.reset();
    }

    fn reset(&mut self) {
        for channel in &mut self.filters {
            for filter in channel.iter_mut() {
                filter.reset();
            }
        }
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        let frames = buffer.frame_count();
        if frames == 0 {
            return;
        }
        let enabled = self.enabled;
        let coefficients = self.coefficients;

        for (channel, filters) in buffer
            .channels_mut()
            .iter_mut()
            .zip(self.filters.iter_mut())
        {
            let samples = &mut channel[..frames];
            for (band, filter) in filters.iter_mut().enumerate() {
                if enabled[band] {
                    filter.set_coefficients(coefficients[band]);
                    filter.process_block(samples);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::param::gain_to_db;

    const SR: f64 = 48_000.0;

    fn prepared() -> Equalizer {
        let mut plugin = Equalizer::new();
        plugin.prepare(&PrepareContext::new(SR, 4_096, 2));
        plugin
    }

    fn context(frames: usize) -> ProcessContext {
        ProcessContext::realtime(SR, frames, 0, 120.0, true)
    }

    #[test]
    fn reenabled_band_does_not_replay_its_pre_bypass_state() {
        let mut plugin = prepared();
        plugin.set_param_by_key("p1_freq", 1_000.0);
        plugin.set_param_by_key("p1_gain", MAX_GAIN_DB);
        plugin.set_param_by_key("p1_q", MAX_Q);

        let mut impulse = AudioBuffer::stereo(1, SR);
        impulse.channel_mut(0)[0] = 1.0;
        impulse.channel_mut(1)[0] = 1.0;
        plugin.process(&mut impulse, &context(1));

        plugin.set_param_by_key("p1_enabled", 0.0);
        let mut bypassed = AudioBuffer::stereo(64, SR);
        plugin.process(&mut bypassed, &context(64));
        assert_eq!(bypassed.peak(), 0.0);

        plugin.set_param_by_key("p1_enabled", 1.0);
        let mut resumed = AudioBuffer::stereo(64, SR);
        plugin.process(&mut resumed, &context(64));
        assert_eq!(resumed.peak(), 0.0, "the band replayed stale filter memory");
    }

    fn sine(frames: usize, frequency: f32) -> AudioBuffer {
        let step = std::f32::consts::TAU * frequency / SR as f32;
        let samples: Vec<f32> = (0..frames).map(|n| (step * n as f32).sin()).collect();
        AudioBuffer::from_planar(vec![samples.clone(), samples], SR).unwrap()
    }

    /// Level change measured over the settled half of a one second tone.
    fn measured_db(plugin: &mut Equalizer, frequency: f32) -> f32 {
        let frames = 48_000;
        let mut buffer = sine(frames, frequency);
        plugin.process(&mut buffer, &context(frames));
        let tail = buffer.slice(frames / 2, frames / 2);
        // A unit sine has an RMS of 1/sqrt(2), which is the reference here.
        gain_to_db(tail.channel_rms(0) * std::f32::consts::SQRT_2)
    }

    #[test]
    fn defaults_are_transparent() {
        let mut plugin = prepared();
        for frequency in [50.0, 500.0, 5_000.0] {
            assert!(plugin.magnitude_db(frequency).abs() < 1e-3);
            assert!(measured_db(&mut plugin, frequency).abs() < 0.01);
        }
    }

    #[test]
    fn peaking_band_reaches_its_gain_at_its_centre_frequency() {
        let mut plugin = prepared();
        plugin.set_param_by_key("p1_enabled", 1.0);
        plugin.set_param_by_key("p1_freq", 1_000.0);
        plugin.set_param_by_key("p1_q", 2.0);
        for gain_db in [-9.0f32, -3.0, 6.0, 12.0] {
            plugin.set_param_by_key("p1_gain", gain_db);
            plugin.reset();
            let predicted = plugin.magnitude_db(1_000.0);
            assert!(
                (predicted - gain_db).abs() < 0.5,
                "curve said {predicted} dB for a {gain_db} dB band"
            );
            let measured = measured_db(&mut plugin, 1_000.0);
            assert!(
                (measured - gain_db).abs() < 0.5,
                "audio said {measured} dB for a {gain_db} dB band"
            );
        }
    }

    #[test]
    fn one_khz_survives_a_hundred_hertz_high_pass() {
        let mut plugin = prepared();
        plugin.set_param_by_key("hp_enabled", 1.0);
        plugin.set_param_by_key("hp_freq", 100.0);
        let measured = measured_db(&mut plugin, 1_000.0);
        assert!(measured > -0.1, "1 kHz lost {measured} dB");

        plugin.reset();
        // A decade below the corner the same filter is 40 dB down.
        let measured = measured_db(&mut plugin, 10.0);
        assert!(
            (measured + 40.0).abs() < 1.5,
            "10 Hz measured {measured} dB, expected about -40"
        );
    }

    #[test]
    fn one_khz_survives_a_ten_khz_low_pass() {
        let mut plugin = prepared();
        plugin.set_param_by_key("lp_enabled", 1.0);
        plugin.set_param_by_key("lp_freq", 10_000.0);
        let measured = measured_db(&mut plugin, 1_000.0);
        assert!(measured > -0.1, "1 kHz lost {measured} dB");
        assert!((plugin.magnitude_db(1_000.0) - measured).abs() < 0.1);
    }

    #[test]
    fn the_curve_sums_every_enabled_band() {
        let mut plugin = prepared();
        plugin.set_param_by_key("p1_freq", 1_000.0);
        plugin.set_param_by_key("p1_gain", 6.0);
        plugin.set_param_by_key("p1_q", 3.0);
        plugin.set_param_by_key("p2_freq", 1_000.0);
        plugin.set_param_by_key("p2_gain", 4.0);
        plugin.set_param_by_key("p2_q", 3.0);
        assert!((plugin.magnitude_db(1_000.0) - 10.0).abs() < 0.5);

        // A disabled band must drop out of the curve entirely.
        plugin.set_param_by_key("p2_enabled", 0.0);
        assert!((plugin.magnitude_db(1_000.0) - 6.0).abs() < 0.5);
    }

    #[test]
    fn the_curve_matches_audio_with_every_band_kind_engaged() {
        // `defaults_are_transparent` and the peaking test only exercise the flat case and one
        // bell. Drive all six kinds at once, including both shelves and both pass filters, and
        // check the UI curve against audio actually pushed through the chain.
        let mut plugin = Equalizer::new();
        plugin.prepare(&PrepareContext::new(SR, 48_000, 2));
        for (key, value) in [
            ("hp_enabled", 1.0f32),
            ("hp_freq", 80.0),
            ("ls_freq", 200.0),
            ("ls_gain", 6.0),
            ("p1_freq", 800.0),
            ("p1_gain", -8.0),
            ("p1_q", 2.0),
            ("p2_freq", 2_500.0),
            ("p2_gain", 5.0),
            ("hs_freq", 7_000.0),
            ("hs_gain", -4.0),
            ("lp_enabled", 1.0),
            ("lp_freq", 14_000.0),
        ] {
            assert!(plugin.set_param_by_key(key, value), "unknown key {key}");
        }

        for frequency in [60.0f32, 150.0, 400.0, 800.0, 2_500.0, 6_000.0, 12_000.0] {
            plugin.reset();
            let measured = measured_db(&mut plugin, frequency);
            let predicted = plugin.magnitude_db(frequency);
            assert!(
                (measured - predicted).abs() < 0.2,
                "{frequency} Hz: curve said {predicted} dB, audio said {measured} dB"
            );
        }
    }

    #[test]
    fn a_low_pass_above_nyquist_stays_stable() {
        let mut plugin = Equalizer::new();
        plugin.prepare(&PrepareContext::new(44_100.0, 512, 2));
        plugin.set_param_by_key("lp_enabled", 1.0);
        plugin.set_param_by_key("lp_freq", 20_000.0);
        let step = std::f32::consts::TAU * 15_000.0 / 44_100.0;
        let samples: Vec<f32> = (0..44_100).map(|n| (step * n as f32).sin()).collect();
        let mut buffer =
            AudioBuffer::from_planar(vec![samples.clone(), samples], 44_100.0).unwrap();
        plugin.process(
            &mut buffer,
            &ProcessContext::realtime(44_100.0, 44_100, 0, 120.0, true),
        );
        assert!(buffer.peak() < 2.0, "peaked at {}", buffer.peak());
        assert!(buffer.channel(0).iter().all(|s| s.is_finite()));
    }

    #[test]
    fn the_layout_names_parameters_the_plugin_actually_has() {
        // An editor finds a band's four controls by these keys. A key that named nothing would
        // leave that band off the curve entirely, and there is no way to see that from the table.
        let plugin = Equalizer::new();
        for (band, entry) in LAYOUT.iter().enumerate() {
            assert_eq!(Some(entry.kind), Equalizer::band_kind(band));
            assert_eq!(entry.label, Equalizer::band_label(band));
            for key in entry.keys {
                assert!(
                    plugin.parameters().iter().any(|p| p.key == key),
                    "{key} is on the display and not on the plugin"
                );
            }
        }
        // In the order the display reads them, which is the order they are documented in: a
        // frequency taken for a gain would draw every node against the wrong axis.
        assert_eq!(LAYOUT[2].keys, ["p1_enabled", "p1_freq", "p1_gain", "p1_q"]);
    }

    #[test]
    fn only_the_shapes_with_a_level_have_a_gain() {
        assert!(!EqBandKind::HighPass.has_gain());
        assert!(!EqBandKind::LowPass.has_gain());
        for kind in [
            EqBandKind::LowShelf,
            EqBandKind::Peaking,
            EqBandKind::HighShelf,
        ] {
            assert!(kind.has_gain());
        }

        // And the claim is about the audio, not about the display: a pass filter handed a large
        // gain has to sound exactly like one handed none, or a node that refuses to move
        // vertically would be hiding a change rather than preventing one.
        let mut plugin = prepared();
        plugin.set_param_by_key("hp_enabled", 1.0);
        plugin.set_param_by_key("hp_freq", 200.0);
        let flat = plugin.magnitude_db(100.0);
        plugin.set_param_by_key("hp_gain", 18.0);
        assert_eq!(plugin.magnitude_db(100.0), flat);
    }

    #[test]
    fn the_curve_a_display_draws_is_the_curve_the_plugin_makes() {
        // Two pieces of code reading the same table: the plugin caches coefficients as parameters
        // arrive, and `response_db` designs them on the spot from settings an editor is holding.
        // They have to agree at every frequency or the graph is a drawing of something else.
        let mut plugin = prepared();
        let settings = [
            ("hp", EqBandKind::HighPass, true, 80.0, 0.0, 0.707),
            ("ls", EqBandKind::LowShelf, true, 200.0, 6.0, 0.707),
            ("p1", EqBandKind::Peaking, true, 800.0, -8.0, 2.0),
            ("p2", EqBandKind::Peaking, false, 2_500.0, 5.0, 1.0),
            ("hs", EqBandKind::HighShelf, true, 7_000.0, -4.0, 0.707),
            ("lp", EqBandKind::LowPass, true, 14_000.0, 0.0, 0.707),
        ];
        let bands: Vec<EqBandSetting> = settings
            .iter()
            .map(|&(prefix, kind, enabled, frequency, gain_db, q)| {
                for (suffix, value) in [
                    ("_enabled", f32::from(u8::from(enabled))),
                    ("_freq", frequency),
                    ("_gain", gain_db),
                    ("_q", q),
                ] {
                    assert!(plugin.set_param_by_key(&format!("{prefix}{suffix}"), value));
                }
                EqBandSetting {
                    kind,
                    enabled,
                    frequency,
                    gain_db,
                    q,
                }
            })
            .collect();

        for frequency in [30.0f32, 60.0, 200.0, 800.0, 2_500.0, 7_000.0, 18_000.0] {
            let drawn = response_db(&bands, SR, frequency);
            let played = plugin.magnitude_db(frequency);
            assert!(
                (drawn - played).abs() < 1e-3,
                "{frequency} Hz: the display says {drawn} dB and the plugin {played} dB"
            );
        }

        // A band that is switched off drops out of both, which is what lets a display draw the
        // curve a user is about to hear rather than the one every band would make together.
        assert_eq!(response_db(&[], SR, 1_000.0), 0.0);
    }

    #[test]
    fn silence_stays_silent() {
        let mut plugin = prepared();
        plugin.set_param_by_key("p1_gain", 24.0);
        plugin.set_param_by_key("hp_enabled", 1.0);
        let mut buffer = AudioBuffer::stereo(512, SR);
        plugin.process(&mut buffer, &context(512));
        assert_eq!(buffer.peak(), 0.0);
    }
}
