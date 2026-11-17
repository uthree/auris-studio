//! Setting a mix's levels by rendering it and measuring what came out.
//!
//! A fader position is not a level. What a track is actually worth depends on the instrument that
//! answered — a General MIDI piano out of one font against a chiptune square out of the built-in
//! synth, at the same number on the same fader, are not within ten decibels of each other — and on
//! how many notes it plays and where they sit. Every level in this program that was decided
//! without listening is a guess about that, and the guesses have been drifting for as long as
//! there have been two ways to make a sound.
//!
//! So this listens. Each track is rendered alone, measured with [`auris_dsp::integrated_lufs`],
//! and its fader moved to put it where its part is supposed to sit; then the whole mix is rendered
//! and every fader moved together until the piece lands on [`TARGET_LUFS`]. The second half is one
//! offset applied to all of them rather than a master trim, because the master's fader is *after*
//! its effects — a limiter cannot hold down a level that is added after it.
//!
//! Where a part is supposed to sit is
//! [`MixerStrip::target_lufs`](auris_core::project::MixerStrip::target_lufs), set by whoever
//! knows what a track is for. A strip with no target — anything a person made by hand — keeps its
//! fader and travels with the offset, so running this over an ordinary project is a loudness
//! normalisation and nothing more.
//! That is the honest limit of it, and it is a limit about knowledge rather than about arithmetic.

use auris_core::param::db_to_gain;
use auris_core::plugin::{PrepareContext, ProcessContext};
use auris_core::project::{EffectSlot, Tap};
use auris_core::registry::PluginRegistry;
use auris_core::{AudioBuffer, EffectSlotId, Project, TrackId};
use auris_dsp::integrated_lufs;
use auris_engine::EngineCommand;
use auris_engine::OfflineOptions;
use auris_gpu::analysis::analyze_loudness;
#[cfg(test)]
use auris_gpu::analysis::analyze_loudness_cpu;

use crate::error::SessionError;
use crate::{Edit, ParamTarget};

use super::Session;

#[path = "levels_job.rs"]
mod job;
pub use job::{
    ComposeBalanceJob, ComposeBalancePhase, ComposeBalanceProgress, ComposeBalanceResult,
    ComposeBalanceStep,
};

/// Where a finished piece is aimed, in LUFS.
///
/// Fourteen under full scale, which is where every streaming service normalises to and therefore
/// where a piece that is going anywhere will end up anyway. Aiming at it here means the level a
/// listener hears is the one the mix was made at, rather than one a normaliser worked out
/// afterwards by turning the whole thing down.
pub const TARGET_LUFS: f32 = -14.0;

/// How far a fader can travel, in decibels.
///
/// The same range the mixer offers, because a level this works out has to be one a person can then
/// see and turn — see `Session::mixer_descriptor`.
const FADER_RANGE_DB: (f32, f32) = (-60.0, 12.0);

/// Where the estimated true peak of the finished mix is held, in dBFS.
///
/// The composed master's own limiter ceiling. A piece that has no limiter on it is held to the
/// same place by this arithmetic instead, which is why the number lives here rather than being
/// read off whatever effect happens to be in the chain.
pub const CEILING_DB: f32 = -0.3;

/// The effect a finished mix is held under its ceiling by.
///
/// Every new document carries one across its master, and so does every composed piece. A mix
/// with nothing on its master can only be made as loud as its single loudest moment allows, and a
/// drum hit is fifteen to twenty decibels over the average of the music around it: a song written
/// note by note and normalised without one came out at -19.7 LUFS against a -14 target, because
/// one snare had already reached the ceiling. The limiter is what lets that one moment give a few
/// decibels so that everything else does not have to.
pub(crate) const MASTER_LIMITER: &str = "auris.fx.limiter";

/// The limiter parameter that sets the level nothing may pass.
pub(crate) const LIMITER_CEILING: &str = "ceiling_db";

/// The limiter parameter that drives the signal into it.
const LIMITER_INPUT: &str = "input_db";

/// Where a new document's master limiter is set, in dBTP.
///
/// One decibel under full scale, the true-peak ceiling the streaming services ask for — and the
/// one `normalize_mix` defaults to, so that a mix made in a new document and normalised with no
/// arguments is held by the limiter it already has rather than by arithmetic about its peaks.
pub(crate) const NEW_DOCUMENT_CEILING_DB: f32 = -1.0;

/// Puts a limiter at the end of `project`'s master chain, its ceiling written down.
///
/// Written into the document as a slot anyone can see and take off, rather than clamped somewhere
/// in the renderer. It is a mixing decision, and a mixing decision a person cannot find is one
/// they cannot disagree with. `false` when the registry has no limiter to offer, which leaves the
/// project as it was.
pub(crate) fn install_master_limiter(
    registry: &PluginRegistry,
    project: &mut Project,
    ceiling_db: f32,
) -> bool {
    if !registry.has_effect(MASTER_LIMITER) {
        return false;
    }
    let Some(slot) = project.add_effect(None, MASTER_LIMITER) else {
        return false;
    };
    if let Some(added) = project.master.effects.iter_mut().find(|s| s.id == slot) {
        // Written down rather than left to the plugin's default, so the ceiling the piece is
        // measured against is the ceiling the file carries.
        added
            .state
            .params
            .insert(LIMITER_CEILING.to_string(), ceiling_db);
    }
    true
}

/// The fader that puts a track measuring `measured` onto `target`.
///
/// The measurement was taken *through* the fader the track is set to, so what is being solved for
/// is the move rather than the level: a track reading -30 LUFS at -12 dB is worth -18 on its own,
/// and where the fader ends up says nothing about either number by itself.
pub fn fader_for(target_lufs: f32, measured_lufs: f32, current_db: f32) -> f32 {
    (current_db + (target_lufs - measured_lufs)).clamp(FADER_RANGE_DB.0, FADER_RANGE_DB.1)
}

/// The most the master limiter may be left to take off the loudest moment, in decibels.
///
/// A limiter catching the odd transient is mastering; a limiter holding down three decibels of
/// everything is a mix played through a wall. It is also the only way a dense piece reaches the
/// target at all — the last four decibels of every record made since about 1995 are this — so the
/// question is not whether to use it but how far.
pub const LIMITER_ALLOWANCE_DB: f32 = 3.0;

/// The most `normalize_mix` lets the master limiter take off the loudest moment, in decibels.
///
/// Twice [`LIMITER_ALLOWANCE_DB`], because the two passes are asked different things. The
/// composer balances a piece it wrote to a calibrated table and stops a little quiet rather than
/// lean on the limiter; `normalize_mix` is asked for a number, by an agent or a person who has
/// already decided the piece is finished. A song written note by note with an ordinary kit has
/// its snare eighteen decibels over its average, so three decibels left it at -17 LUFS against a
/// -14 target, still audibly quieter than whatever plays next to it. Six is what that song needed,
/// and it is still transient work for a two-millisecond look-ahead: what a dense, sustained mix
/// asks of the limiter is less, because its loudest moment is nearer its average to begin with.
pub const NORMALIZE_LIMITER_ALLOWANCE_DB: f32 = 6.0;

/// How far every fader goes up together, once the parts are balanced against each other.
///
/// The least of three: what the loudness asks for, what leaves the limiter no more than
/// [`LIMITER_ALLOWANCE_DB`] to take off the loudest moment, and what the faders themselves have
/// left before the top of their travel. Never below zero — a mix that is *over* the target comes
/// down on the master, where there is nothing to run out of.
///
/// The third term is the one worth explaining. Lifting every fader keeps the balance only while
/// every fader can actually move; the moment one of them hits the top, the mix quietly stops being
/// the mix that was just measured. Stopping short of that is choosing a piece that is a little
/// quiet over a piece that is wrong, and the quiet is recoverable in one gesture.
pub fn faders_lift_db(measured_lufs: f32, true_peak_db: f32, headroom_db: f32) -> f32 {
    let wanted = TARGET_LUFS - measured_lufs;
    let into_the_limiter = CEILING_DB + LIMITER_ALLOWANCE_DB - true_peak_db;
    wanted.min(into_the_limiter).min(headroom_db).max(0.0)
}

/// Where the master fader goes, given what the lifted mix measured.
///
/// Whichever is less: the lift the loudness asks for, or the lift that still leaves the loudest
/// moment under [`CEILING_DB`]. Nothing catches what this adds — the master's own effects run
/// *before* its fader, so a limiter in the chain has already done whatever it was going to do —
/// and a piece that reached the target by clipping would have reached it by breaking.
///
/// A mix that is *over* the target comes down by the whole distance: there is no such thing as too
/// much headroom, and the ceiling cannot bind on the way down.
///
/// `true_peak_db` estimates the reconstructed waveform's peak rather than the loudest sample,
/// because that is what the converter downstream will actually meet.
pub fn master_gain_db(measured_lufs: f32, true_peak_db: f32) -> f32 {
    let wanted = TARGET_LUFS - measured_lufs;
    let headroom = CEILING_DB - true_peak_db;
    wanted
        .min(headroom.max(0.0))
        .clamp(FADER_RANGE_DB.0, FADER_RANGE_DB.1)
}

/// What one track measured, and what was done about it.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackLevel {
    /// The track's name, for a report a person reads.
    pub name: String,
    /// Where it was aiming, or `None` for a strip nobody has aimed.
    pub target_lufs: Option<f32>,
    /// What it measured on its own, in LUFS, or `None` if it made no sound.
    pub measured_lufs: Option<f32>,
    /// Where its fader was.
    pub was_db: f32,
    /// Where its fader ended up.
    pub now_db: f32,
}

impl TrackLevel {
    /// What this part will measure at its new fader, in LUFS.
    ///
    /// Worked out rather than rendered a second time, and exactly right: a fader is a gain, and
    /// moving a signal by a decibel moves every measurement of it by a decibel. Nothing between
    /// here and the master can argue — the strip's own effects are before the fader, and the
    /// limiter that could argue is on the master and is dealt with there.
    pub fn reached_lufs(&self) -> Option<f32> {
        self.measured_lufs
            .map(|lufs| lufs + (self.now_db - self.was_db))
    }
}

/// What balancing a mix did.
#[derive(Clone, Debug, PartialEq)]
pub struct BalanceReport {
    /// One entry per track that carries a level, in track order.
    pub tracks: Vec<TrackLevel>,
    /// How far every fader went up together once the parts were balanced, in decibels.
    ///
    /// The same number on all of them, so it changes how loud the piece is and not how it is
    /// balanced. A part's target is where it sits *before* this — see [`Self::short_by_db`].
    pub lift_db: f32,
    /// Where the master fader ended up, in decibels.
    pub master_db: f32,
    /// What the mix measured with the parts balanced and the master still where it was.
    ///
    /// Not what the piece measured before any of this: that would be a third render of the whole
    /// arrangement, for a number nobody acts on. What this is good for is the one thing it is used
    /// for — working out where the master goes.
    pub balanced_lufs: Option<f32>,
    /// What it measures now, rendered again and measured rather than predicted.
    pub now_lufs: Option<f32>,
}

/// Measured result of moving every source fader by one common offset.
#[derive(Clone, Debug, PartialEq)]
pub struct NormalizeMixReport {
    /// Requested integrated loudness in LUFS.
    pub target_lufs: f32,
    /// Requested true-peak ceiling in dBTP.
    pub ceiling_db: f32,
    /// Original whole-mix loudness and peak.
    pub before: super::MixAnalysis,
    /// Measured whole-mix loudness and peak after the change.
    pub after: super::MixAnalysis,
    /// Shared change applied to every source fader in dB.
    pub offset_db: f32,
    /// Whether the master limiter held the ceiling, its own ceiling set to the requested one.
    ///
    /// `false` when the master chain does not end in a limiter: the loudest moment then had to
    /// stay under the ceiling by itself, and a mix with sharp drums stops well short of loud.
    pub limited: bool,
}

/// The limiter at the end of the master chain, as `normalize_mix` reads it.
struct MasterLimiter {
    slot: EffectSlotId,
    ceiling: ParamTarget,
    input_db: f32,
}

impl BalanceReport {
    /// How far the loudest part is from where it was aiming, in decibels.
    ///
    /// Zero when every part reached its target, which is the usual answer. Anything else is a
    /// fader that ran out of travel — a part the instrument playing it cannot make loud enough —
    /// and it is worth saying out loud, because the mix is then as balanced as a mixer can make it
    /// rather than as balanced as it should be.
    pub fn short_by_db(&self) -> f32 {
        self.tracks
            .iter()
            .filter_map(|level| Some(level.target_lufs? + self.lift_db - level.reached_lufs()?))
            .fold(0.0f32, |worst, short| worst.max(short))
    }
}

impl Session {
    /// Adjusts overall loudness while preserving source-fader differences.
    ///
    /// The target is approached without exceeding the measured true-peak ceiling or the
    /// faders' travel. Master gain and any authored automation remain untouched. Refuses
    /// automated source gain, which would make the common offset ineffective.
    ///
    /// One render where it can: the signal arriving at a master limiter, which the limiter is
    /// then run over here for the mix before and, where everything after the source faders
    /// scales with them, for the mix after. A master that does more after its effects than a
    /// fixed gain, or a stage that answers a louder input with something other than a louder
    /// output, puts back a render for each measurement it spoils.
    pub fn normalize_mix(
        &mut self,
        target_lufs: f32,
        ceiling_db: f32,
    ) -> Result<NormalizeMixReport, SessionError> {
        if !target_lufs.is_finite() || !(-60.0..=-6.0).contains(&target_lufs) {
            return Err(SessionError::MixNormalization(
                "target_lufs must be finite and between -60 and -6".into(),
            ));
        }
        if !ceiling_db.is_finite() || !(-12.0..=0.0).contains(&ceiling_db) {
            return Err(SessionError::MixNormalization(
                "ceiling_db must be finite and between -12 and 0".into(),
            ));
        }
        let faders: Vec<_> = self
            .project
            .tracks
            .iter()
            .filter(|track| !track.kind.is_bus())
            .map(|track| (track.id, track.mixer.gain_db))
            .collect();
        if faders.is_empty() {
            return Err(SessionError::MixNormalization("no source tracks".into()));
        }
        if faders
            .iter()
            .any(|(id, _)| self.is_automated(ParamTarget::TrackGain(*id)))
        {
            return Err(SessionError::MixNormalization(
                "a source gain is automated; clear its gain lane before normalizing".into(),
            ));
        }
        // One render serves every measurement where it can: the signal arriving at the master
        // limiter, the limiter then run over it here. Without that signal, each measurement is
        // a render of its own.
        let limiter = self.leaning_limiter();
        let arriving = match &limiter {
            Some(limiter) => self.limiter_input(limiter)?,
            None => None,
        };
        let (before, dry_peak, plain) = match (&limiter, &arriving) {
            (Some(limiter), Some(arriving)) => {
                let mix = self.through_master_limiter(limiter.slot, arriving, 1.0)?;
                let master = self.project.master.gain_db;
                let dry = analyze_loudness(self.gpu.as_deref(), arriving).true_peak_db() + master;
                (self.analysis_of(&mix, false)?, Some(dry), None)
            }
            (Some(limiter), None) => (
                self.analyze(false)?,
                Some(self.true_peak_without(limiter.slot)?),
                None,
            ),
            (None, _) => {
                let mix = self.render_job().render(
                    &OfflineOptions::whole_project(),
                    &mut auris_engine::RenderProgress::default(),
                )?;
                (self.analysis_of(&mix, false)?, None, Some(mix))
            }
        };
        let Some(lufs) = before.lufs else {
            return Err(SessionError::MixNormalization("the mix is silent".into()));
        };
        // With a limiter last on the master, the ceiling is the limiter's to keep and the peaks
        // may be pushed into it by up to [`NORMALIZE_LIMITER_ALLOWANCE_DB`]; without one, the
        // loudest moment as it stands is the whole limit.
        let peak_room = match (&limiter, dry_peak) {
            // The limiter takes off `pre + input + offset - (ceiling - master)` at the loudest
            // moment, where `pre` is that moment measured with the limiter out at the master's
            // own fader. Holding that to the allowance, the master gain cancels.
            (Some(limiter), Some(dry)) => {
                NORMALIZE_LIMITER_ALLOWANCE_DB + ceiling_db - limiter.input_db - dry
            }
            _ => ceiling_db - before.true_peak_db,
        };
        let desired = (target_lufs - lufs).min(peak_room);
        let lower = faders
            .iter()
            .map(|(_, gain)| FADER_RANGE_DB.0 - gain)
            .fold(f32::NEG_INFINITY, f32::max);
        let upper = faders
            .iter()
            .map(|(_, gain)| FADER_RANGE_DB.1 - gain)
            .fold(f32::INFINITY, f32::min);
        let offset_db = desired.clamp(lower, upper);
        self.begin_transaction(Edit::BalanceLevels);
        if let Some(limiter) = &limiter {
            let held = (ceiling_db - self.project.master.gain_db).clamp(-24.0, 0.0);
            self.set_param(limiter.ceiling, held);
        }
        for (id, gain) in faders {
            self.set_param(ParamTarget::TrackGain(id), gain + offset_db);
        }
        // Moving every source fader by one offset scales everything up to the first stage that
        // is not proportional by the same factor. Where nothing before the limiter (or, without
        // one, before the output) is such a stage, the new mix is the old signal scaled and run
        // through the limiter's new ceiling, and needs no render of its own.
        let linear = self.scales_with_a_common_fader_offset(limiter.as_ref().map(|l| l.slot));
        let offset = db_to_gain(offset_db);
        let measured = match (&limiter, &arriving, plain) {
            (Some(limiter), Some(arriving), _) if linear => self
                .through_master_limiter(limiter.slot, arriving, offset)
                .and_then(|mix| self.analysis_of(&mix, false)),
            (None, _, Some(mut mix)) if linear => {
                mix.apply_gain(offset);
                self.analysis_of(&mix, false)
            }
            _ => self.analyze(false),
        };
        let after = match measured {
            Ok(after) => after,
            Err(error) => {
                self.revert_transaction();
                return Err(error);
            }
        };
        self.end_transaction();
        Ok(NormalizeMixReport {
            target_lufs,
            ceiling_db,
            before,
            after,
            offset_db,
            limited: limiter.is_some(),
        })
    }

    /// What arrives at the master limiter, rendered once: the limiter left out and the master
    /// fader at unity, so [`Self::through_master_limiter`] can make the mix from it.
    ///
    /// `None` where the master does more after its effects than a fixed gain (a pan off
    /// centre, a mute, or an automated lane on the master or its effects), since that stage
    /// would then have to be rebuilt here as well.
    fn limiter_input(
        &mut self,
        limiter: &MasterLimiter,
    ) -> Result<Option<AudioBuffer>, SessionError> {
        let master = &self.project.master;
        let master_automated = self.project.automation.lanes().iter().any(|lane| {
            matches!(
                lane.target,
                ParamTarget::MasterGain
                    | ParamTarget::MasterPan
                    | ParamTarget::Effect { track: None, .. }
            )
        });
        if master.pan != 0.0 || master.mute || master_automated || !master.gain_db.is_finite() {
            return Ok(None);
        }
        let mut project = self.project.clone();
        project.master.gain_db = 0.0;
        if let Some(effect) = project
            .master
            .effects
            .iter_mut()
            .find(|s| s.id == limiter.slot)
        {
            effect.enabled = false;
        }
        let arriving = self.job_for(project).render(
            &OfflineOptions::whole_project(),
            &mut auris_engine::RenderProgress::default(),
        )?;
        Ok(Some(arriving))
    }

    /// The mix a listener hears, made from [`Self::limiter_input`]'s signal scaled by `gain`:
    /// the master limiter as the document now sets it, run here, then the master fader.
    ///
    /// The engine holds the whole mix back by the limiter's look-ahead and drops that lead-in
    /// from an export, so this does too: the signal runs on past its end by that many frames
    /// of silence, and the same number come off the front.
    fn through_master_limiter(
        &self,
        slot: EffectSlotId,
        arriving: &AudioBuffer,
        gain: f32,
    ) -> Result<AudioBuffer, SessionError> {
        let stored = self
            .project
            .master
            .effects
            .iter()
            .find(|s| s.id == slot)
            .ok_or_else(|| SessionError::MixNormalization("the master limiter is gone".into()))?;
        let mut limiter = self
            .registry
            .create_effect(&stored.effect_id)
            .map_err(|error| SessionError::MixNormalization(error.to_string()))?;
        limiter.load_state(&stored.state);
        let rate = arriving.sample_rate();
        let channels = arriving.channel_count();
        let block = self.engine.max_block().max(1);
        limiter.prepare(&PrepareContext::new(rate, block, channels));
        let latency = limiter.latency_frames();
        let frames = arriving.frame_count();
        let master = db_to_gain(self.project.master.gain_db);
        let mut out = AudioBuffer::new(channels, frames, rate);
        let (mut written, mut position) = (0usize, 0usize);
        while written < frames {
            let length = block.min(frames + latency - position);
            let mut chunk_view = AudioBuffer::new(channels, length, rate);
            for (into, from) in chunk_view
                .channels_mut()
                .iter_mut()
                .zip(arriving.channels())
            {
                for (frame, sample) in into.iter_mut().enumerate() {
                    *sample = from.get(position + frame).map_or(0.0, |value| value * gain);
                }
            }
            let context = ProcessContext {
                sample_rate: rate,
                block_frames: length,
                playhead_samples: position as u64,
                bpm: 120.0,
                is_playing: true,
                is_offline: true,
            };
            limiter.process(&mut chunk_view, &context);
            let skip = latency.saturating_sub(position).min(length);
            let take = (length - skip).min(frames - written);
            for (into, from) in out.channels_mut().iter_mut().zip(chunk_view.channels()) {
                for (offset, value) in from[skip..skip + take].iter().enumerate() {
                    into[written + offset] = value * master;
                }
            }
            written += take;
            position += length;
        }
        Ok(out)
    }

    /// Whether moving every source fader by one offset scales the signal by that offset all
    /// the way to the master's `stop` slot (or to the output, with none).
    ///
    /// The inserts on a source track sit before its fader and do not matter. What does is
    /// everything after it: an output tapped before the fader keeps its level, an effect keyed
    /// from another track hears a key that changed, and a bus or master effect that is not one
    /// of the proportional built-ins (an equaliser, a delay, a reverb, a chorus, a gain)
    /// answers a louder input with something other than a louder output.
    fn scales_with_a_common_fader_offset(&self, stop: Option<EffectSlotId>) -> bool {
        const PROPORTIONAL: [&str; 5] = [
            "auris.fx.eq",
            "auris.fx.delay",
            "auris.fx.reverb",
            "auris.fx.chorus",
            "auris.fx.gain",
        ];
        let proportional = |slot: &&EffectSlot| PROPORTIONAL.contains(&slot.effect_id.as_str());
        let tracks = self.project.tracks.iter().all(|track| {
            let keyed = track
                .mixer
                .effects
                .iter()
                .any(|slot| slot.sidechain.is_some());
            let after_fader = if track.kind.is_bus() {
                track
                    .mixer
                    .effects
                    .iter()
                    .filter(|s| s.enabled)
                    .all(|s| proportional(&s))
            } else {
                track
                    .outputs
                    .iter()
                    .all(|output| output.tap != Tap::PreFader)
            };
            !keyed && after_fader
        });
        let master = self
            .project
            .master
            .effects
            .iter()
            .take_while(|slot| Some(slot.id) != stop)
            .filter(|slot| slot.enabled)
            .all(|s| proportional(&s));
        tracks && master
    }

    /// The master limiter `normalize_mix` can lean on, if there is one it may set the ceiling of.
    ///
    /// Only the last enabled slot of the master chain counts — anything after a limiter could
    /// lift what it held down — and only one whose ceiling nobody has automated, since the pass
    /// writes the requested ceiling into it.
    fn leaning_limiter(&mut self) -> Option<MasterLimiter> {
        let slot = self
            .project
            .master
            .effects
            .iter()
            .rev()
            .find(|s| s.enabled)?;
        if slot.effect_id != MASTER_LIMITER {
            return None;
        }
        let (id, stored_input) = (slot.id, slot.state.params.get(LIMITER_INPUT).copied());
        let descriptors = self.effect_descriptors(id);
        let param = descriptors
            .iter()
            .find(|descriptor| descriptor.key == LIMITER_CEILING)?
            .id;
        let ceiling = ParamTarget::Effect {
            track: None,
            slot: id,
            param,
        };
        if self.is_automated(ceiling) {
            return None;
        }
        let input_db = descriptors
            .iter()
            .find(|descriptor| descriptor.key == LIMITER_INPUT)
            .map_or(0.0, |descriptor| stored_input.unwrap_or(descriptor.default));
        Some(MasterLimiter {
            slot: id,
            ceiling,
            input_db,
        })
    }

    /// The mix's true peak with one master slot switched off, everything else as it stands.
    fn true_peak_without(&mut self, slot: EffectSlotId) -> Result<f32, SessionError> {
        let mut project = self.project.clone();
        if let Some(effect) = project.master.effects.iter_mut().find(|s| s.id == slot) {
            effect.enabled = false;
        }
        let mix = self.job_for(project).render(
            &OfflineOptions::whole_project(),
            &mut auris_engine::RenderProgress::default(),
        )?;
        Ok(analyze_loudness(self.gpu.as_deref(), &mix).true_peak_db())
    }

    /// Renders every track alone, measures it, and sets the mix from what it heard.
    ///
    /// One undo step. Everything it moves is a fader, so taking it back is exactly as cheap as
    /// making it — nothing is written, nothing is resampled, and no note is touched.
    ///
    /// The cost is one render per track that plays, plus two of the whole mix: one to find out
    /// where the piece sits and one to confirm where it ended up. The confirmation is not a
    /// formality — the master limiter may take some of the last lift back, and a report that
    /// predicted the answer instead of measuring it would be claiming the one thing this whole
    /// pass exists to stop anybody claiming.
    pub fn balance_levels(&mut self) -> Result<BalanceReport, SessionError> {
        let mut job = self.begin_balance_levels_job();
        loop {
            let result = job.run(&mut auris_engine::RenderProgress::default())?;
            match self.continue_composed_balance(result)? {
                ComposeBalanceStep::Pending(next) => job = next,
                ComposeBalanceStep::Complete(report) => return Ok(report),
            }
        }
    }

    /// Moves a fader without recording a step of its own.
    ///
    /// A fader is one of the few things that reaches the audio thread without a rebuild, so the
    /// engine is told rather than left to find out — writing the document alone would leave what
    /// is being played and what is written down disagreeing until the next structural edit.
    ///
    /// `Session::set_param` is the one every other caller wants; it records, and this is called
    /// dozens of times inside one step that has already been recorded.
    fn write_fader(&mut self, track: TrackId, gain_db: f32) {
        let Some(index) = self.project.track_index(track) else {
            return;
        };
        self.project.tracks[index].mixer.gain_db = gain_db;
        self.send(EngineCommand::SetTrackGain { index, gain_db });
    }

    /// The loudness of `track` playing on its own, or `None` if it made no sound.
    ///
    /// Soloed rather than rendered by itself, so that the buses it feeds stay open: what a part is
    /// worth in the mix includes the room it is sent to, and a stem measured dry would put every
    /// part that carries reverb a little too loud.
    pub(super) fn measure_alone(&mut self, track: TrackId) -> Result<Option<f32>, SessionError> {
        let mut alone = self.project.clone();
        for entry in &mut alone.tracks {
            entry.mixer.solo = entry.id == track;
            // A mute survives a solo in this document, and here it must not: the question being
            // asked is what this part is worth, and a part that is muted today is still worth
            // whatever it is worth. The copy is thrown away either way.
            entry.mixer.mute = false;
        }
        Ok(integrated_lufs(&self.render_snapshot(alone)?))
    }

    /// Renders `project` whole, offline, at its own rate, with the master fader at unity.
    ///
    /// At unity because every measurement here is of something *upstream* of that fader, and a
    /// measurement that included it would not survive being taken twice: the second pass would
    /// read the lift the first one applied as loudness the parts had, and pull them all back down
    /// by it. Balancing a piece that is already balanced has to be a fixed point, or the command
    /// is one nobody can run without thinking about whether they ran it before.
    fn render_snapshot(
        &mut self,
        mut project: Project,
    ) -> Result<auris_core::AudioBuffer, SessionError> {
        project.master.gain_db = 0.0;
        let mut job = self.job_for(project);
        job.render(
            &OfflineOptions::whole_project(),
            &mut auris_engine::RenderProgress::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_preserves_fader_difference_and_undoes_together() {
        let (mut session, _) = balanced();
        let ids: Vec<_> = session
            .project()
            .tracks
            .iter()
            .filter(|track| !track.kind.is_bus())
            .map(|track| track.id)
            .collect();
        let before: Vec<_> = ids
            .iter()
            .map(|id| session.project().track(*id).unwrap().mixer.gain_db)
            .collect();
        let report = session.normalize_mix(-23.0, -1.0).unwrap();
        assert!(report.offset_db < 0.0, "expected attenuation: {report:?}");
        let after: Vec<_> = ids
            .iter()
            .map(|id| session.project().track(*id).unwrap().mixer.gain_db)
            .collect();
        for (was, now) in before.iter().zip(&after) {
            assert!((now - was - report.offset_db).abs() < 1.0e-4);
        }
        assert!(((after[0] - after[1]) - (before[0] - before[1])).abs() < 1.0e-4);
        assert!(report.after.true_peak_db <= -0.9);
        assert_eq!(session.undo(), Some(Edit::BalanceLevels));
        for (id, was) in ids.iter().zip(&before) {
            assert_eq!(session.project().track(*id).unwrap().mixer.gain_db, *was);
        }
    }

    #[test]
    fn a_fader_moves_by_the_distance_the_measurement_is_out() {
        // A track worth -18 on its own, set to -12, measures -30. Aimed at -20 it wants six back.
        assert!((fader_for(-20.0, -30.0, -12.0) - -2.0).abs() < 1.0e-4);
        // And the fader cannot leave the range the mixer offers, however far out the measurement
        // is: a track that measured silence-adjacent would otherwise ask for a hundred decibels.
        assert_eq!(fader_for(-20.0, -120.0, 0.0), FADER_RANGE_DB.1);
        assert_eq!(fader_for(-60.0, 0.0, 0.0), FADER_RANGE_DB.0);
    }

    #[test]
    fn a_detached_balance_worker_never_writes_live_faders_or_history() {
        use super::super::SessionOptions;

        let mut session = Session::new(SessionOptions::headless()).expect("a headless session");
        session.project.add_audio_track("Part");
        session.forget_history();
        let before = session.project().clone();
        let job = session.begin_balance_levels_job();
        let cancel = std::sync::atomic::AtomicBool::new(true);

        let _detached = job.run(&mut auris_engine::RenderProgress::default().cancelled_by(&cancel));

        assert_eq!(session.project(), &before);
        assert!(
            !session.can_undo(),
            "a worker-only balance left a history step"
        );
    }

    #[test]
    fn the_lift_stops_where_the_limiter_would_start_working() {
        // Far from the ceiling, the loudness is the whole story.
        assert!((master_gain_db(-20.0, -10.0) - 6.0).abs() < 1.0e-4);
        // Near it, the ceiling is: a mix already peaking at -1 dBFS has seven tenths of a decibel
        // before it hits the ceiling, and the 8 dB its loudness asks for is ten times that.
        assert!((master_gain_db(-22.0, -1.0) - 0.7).abs() < 1.0e-4);
        // A mix already over the target comes all the way down, and the headroom never pushes it
        // back up: there is no such thing as too much of that.
        assert!((master_gain_db(-9.0, -0.1) - -5.0).abs() < 1.0e-4);
    }

    /// Four bars written the way an agent writes them: notes at moderate velocities, every fader
    /// at unity, on the built-in instruments so it measures the same on every machine.
    fn written_by_hand(strip_master: bool) -> Session {
        use super::super::SessionOptions;
        use auris_core::project::Note;
        use auris_core::time::Ticks;
        let q = Ticks::QUARTER.0;
        let bars = 4;
        let length = Ticks(q * 4 * bars);
        let mut session =
            Session::new(SessionOptions::headless().with_master_limiter(!strip_master))
                .expect("a headless session");
        let note = |pitch, at, held, velocity| {
            let mut note = Note::new(pitch, Ticks(at), Ticks(held));
            note.velocity = velocity;
            note
        };
        let lead = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(lead, "Lead", Ticks::ZERO, length)
            .unwrap();
        let tune = [72u8, 74, 76, 79, 77, 76, 74, 72];
        let notes = (0..bars * 4)
            .map(|i| note(tune[(i % 8) as usize], i * q, q * 9 / 10, 0.7))
            .collect();
        session.replace_notes(clip, notes).unwrap();
        let chords = session.add_default_instrument_track("Chords").unwrap();
        let clip = session
            .add_midi_clip(chords, "Chords", Ticks::ZERO, length)
            .unwrap();
        let notes = (0..bars)
            .flat_map(|bar| {
                [60u8, 64, 67].map(|pitch| note(pitch, bar * 4 * q, 4 * q - q / 8, 0.6))
            })
            .collect();
        session.replace_notes(clip, notes).unwrap();
        let bass = session.add_default_instrument_track("Bass").unwrap();
        let clip = session
            .add_midi_clip(bass, "Bass", Ticks::ZERO, length)
            .unwrap();
        let notes = (0..bars * 2)
            .map(|i| note(36, i * 2 * q, 2 * q - q / 8, 0.75))
            .collect();
        session.replace_notes(clip, notes).unwrap();
        let drums = session.add_default_drum_track("Drums").unwrap();
        let clip = session
            .add_midi_clip(drums, "Drums", Ticks::ZERO, length)
            .unwrap();
        let mut notes = Vec::new();
        for i in 0..bars * 8 {
            notes.push(note(42, i * q / 2, q / 4, 0.5));
            match i % 4 {
                0 => notes.push(note(36, i * q / 2, q / 4, 0.8)),
                2 => notes.push(note(38, i * q / 2, q / 4, 0.7)),
                _ => {}
            }
        }
        session.replace_notes(clip, notes).unwrap();
        session
    }

    #[test]
    fn a_new_document_ends_its_master_in_a_limiter_at_the_delivery_ceiling() {
        use super::super::SessionOptions;
        let mut session = Session::new(SessionOptions::headless().with_master_limiter(true))
            .expect("a headless session");
        for project in [session.project().clone(), {
            session.new_project();
            session.project().clone()
        }] {
            let last = project.master.effects.last().expect("a master limiter");
            assert_eq!(last.effect_id, MASTER_LIMITER);
            assert_eq!(
                last.state.params.get(LIMITER_CEILING),
                Some(&NEW_DOCUMENT_CEILING_DB)
            );
        }
    }

    /// The measurements `normalize_mix` makes from one render, held against renders of their own.
    fn assert_measures_alike(
        made: &super::super::MixAnalysis,
        rendered: &super::super::MixAnalysis,
    ) {
        let lufs = |analysis: &super::super::MixAnalysis| analysis.lufs.expect("a mix that plays");
        assert!(
            (lufs(made) - lufs(rendered)).abs() < 0.01,
            "{} LUFS made, {} rendered",
            lufs(made),
            lufs(rendered)
        );
        assert!(
            (made.true_peak_db - rendered.true_peak_db).abs() < 0.01,
            "{} dBTP made, {} rendered",
            made.true_peak_db,
            rendered.true_peak_db
        );
        assert!((made.peak_db - rendered.peak_db).abs() < 0.01);
        assert_eq!(made.sections.len(), rendered.sections.len());
    }

    #[test]
    fn one_render_through_the_limiter_measures_what_two_more_renders_would() {
        let mut session = written_by_hand(false);
        let limiter = session.leaning_limiter().map(|limiter| limiter.slot);
        assert!(session.scales_with_a_common_fader_offset(limiter));
        let rendered_before = session.analyze(false).unwrap();
        let report = session.normalize_mix(TARGET_LUFS, -1.0).unwrap();
        assert!(report.limited && report.offset_db.abs() > 1.0, "{report:?}");
        assert_measures_alike(&report.before, &rendered_before);
        assert_measures_alike(&report.after, &session.analyze(false).unwrap());
    }

    #[test]
    fn without_a_limiter_the_lifted_mix_is_the_measured_one_scaled() {
        let mut session = written_by_hand(true);
        // Pull every fader down and aim a decibel above wherever the instruments then put the
        // mix, so the lift is the loudness target's and not the true peak's however loud the
        // built-in voices are this build.
        let faders: Vec<_> = session
            .project
            .tracks
            .iter()
            .map(|track| (track.id, track.mixer.gain_db))
            .collect();
        for (id, gain_db) in faders {
            session.write_fader(id, gain_db - 6.0);
        }
        let rendered_before = session.analyze(false).unwrap();
        let target = rendered_before.lufs.expect("a mix that plays") + 1.0;
        let report = session.normalize_mix(target, -1.0).unwrap();
        assert!(
            !report.limited && report.offset_db.abs() > 0.5,
            "{report:?}"
        );
        assert_measures_alike(&report.before, &rendered_before);
        assert_measures_alike(&report.after, &session.analyze(false).unwrap());
    }

    #[test]
    fn a_stage_that_does_not_scale_with_the_faders_is_rendered_instead() {
        let mut session = written_by_hand(false);
        let limiter = session.leaning_limiter().map(|limiter| limiter.slot);
        // A compressor ahead of the limiter answers a louder mix with less than a louder mix.
        let compressor = session
            .project
            .add_effect(None, "auris.fx.compressor")
            .unwrap();
        let effects = &mut session.project.master.effects;
        let slot = effects.pop().unwrap();
        assert_eq!(slot.id, compressor);
        effects.insert(0, slot);
        assert!(!session.scales_with_a_common_fader_offset(limiter));
        let report = session.normalize_mix(TARGET_LUFS, -1.0).unwrap();
        assert_measures_alike(&report.after, &session.analyze(false).unwrap());

        // So does an output taken before a source's fader.
        let mut session = written_by_hand(false);
        let bus = session.project.add_bus_track("Room");
        let lead = session.project.tracks[0].id;
        session
            .project
            .track_mut(lead)
            .unwrap()
            .outputs
            .push(auris_core::project::Output {
                to: auris_core::project::Destination::Bus(bus),
                gain_db: -6.0,
                tap: Tap::PreFader,
            });
        let limiter = session.leaning_limiter().map(|limiter| limiter.slot);
        assert!(!session.scales_with_a_common_fader_offset(limiter));
    }

    #[test]
    fn normalizing_leans_on_the_master_limiter_instead_of_stopping_at_the_first_drum_hit() {
        let mut bare = written_by_hand(true);
        let without = bare
            .normalize_mix(TARGET_LUFS, -1.0)
            .expect("the mix renders");
        assert!(!without.limited);
        let mut limited = written_by_hand(false);
        let with = limited
            .normalize_mix(TARGET_LUFS, -1.0)
            .expect("the mix renders");
        assert!(
            with.limited,
            "the new document's limiter is the one leaned on"
        );

        let quiet = without.after.lufs.expect("a mix that plays");
        let loud = with.after.lufs.expect("a mix that plays");
        // The bare mix stops where its snare meets the ceiling; the limited one is let past it.
        assert!(
            loud > quiet + 3.0,
            "with a limiter {loud:.1} LUFS, without {quiet:.1}"
        );
        assert!(
            loud > TARGET_LUFS - 2.5,
            "a hand-written mix only reached {loud:.1} LUFS"
        );
        // And the ceiling is the limiter's to keep, which it does.
        assert!(
            with.after.true_peak_db <= -1.0 + 0.05,
            "true peak {:.2} dBTP over a -1 ceiling",
            with.after.true_peak_db
        );
    }

    /// Four bars of two parts on the built-in instruments, written and balanced.
    ///
    /// No SoundFont, so it measures the same on every machine — and short, because every one of
    /// these renders the piece once per part and twice more.
    fn balanced() -> (Session, BalanceReport) {
        use super::super::SessionOptions;
        use auris_core::{ClipPreset, ClipRecipe, Ticks};
        let mut session =
            Session::new(SessionOptions::headless()).expect("a headless session opens");
        session
            .stamp_named_progression("axis", Ticks::ZERO, 4)
            .expect("a known progression");
        for (name, preset, instrument) in [
            ("tune", ClipPreset::Lead, "auris.synth.chiptune"),
            ("low", ClipPreset::Bass, "auris.synth.fm2"),
        ] {
            let track = session
                .add_instrument_track(name, instrument)
                .expect("a built-in instrument");
            session
                .generate_clip(
                    track,
                    Ticks::ZERO,
                    super::super::fixtures::BAR * 4,
                    ClipRecipe::new(preset, 1),
                )
                .expect("a generated clip");
        }
        // Aimed where a tune and a bass usually sit against each other.
        for (track, target) in session.project.tracks.iter_mut().zip([-23.0, -27.0]) {
            track.mixer.target_lufs = Some(target);
        }
        let report = session.balance_levels().expect("the piece renders");
        (session, report)
    }

    #[test]
    fn a_part_ends_up_where_it_was_aiming() {
        let (_, report) = balanced();
        assert_eq!(report.tracks.len(), 2, "both parts carry a level");
        for level in &report.tracks {
            // Plus the lift, which is the piece being made louder rather than this part being put
            // somewhere else: it is the same number on every fader and leaves the balance alone.
            let target = level.target_lufs.expect("an aimed part") + report.lift_db;
            let reached = level.reached_lufs().expect("a part that plays measures");
            assert!(
                (reached - target).abs() < 0.5,
                "`{}` was aiming at {target:.1} LUFS and reached {reached:.1}",
                level.name
            );
        }
        assert_eq!(report.short_by_db(), 0.0, "nothing ran out of fader");
    }

    #[test]
    fn an_explicit_balance_is_one_reversible_history_step() {
        let (mut session, _) = balanced();
        let balanced = session.project().clone();

        assert_eq!(session.undo(), Some(crate::Edit::BalanceLevels));
        assert_ne!(session.project(), &balanced);
        assert_eq!(session.redo(), Some(crate::Edit::BalanceLevels));
        assert_eq!(session.project(), &balanced);
    }

    #[test]
    fn balancing_a_balanced_mix_moves_nothing() {
        // The property that makes this a command rather than a trick: whether it has been run
        // before is not something anybody should have to remember. It holds because every
        // measurement is taken with the master at unity, so the second pass does not read the
        // first pass's own lift as loudness the parts had.
        let (mut session, first) = balanced();
        let again = session.balance_levels().expect("the piece renders again");
        for (before, after) in first.tracks.iter().zip(&again.tracks) {
            assert!(
                (after.now_db - before.now_db).abs() < 0.2,
                "`{}` moved from {:.2} to {:.2} dB on a second balance",
                after.name,
                before.now_db,
                after.now_db
            );
        }
        assert!(
            (again.master_db - first.master_db).abs() < 0.2,
            "the master moved from {:.2} to {:.2} dB on a second balance",
            first.master_db,
            again.master_db
        );
    }

    #[test]
    fn a_piece_lands_near_the_target_and_never_over_the_ceiling() {
        let (_, report) = balanced();
        let reached = report.now_lufs.expect("a piece that plays measures");
        assert!(
            reached <= TARGET_LUFS + 0.5,
            "the piece came out at {reached:.1} LUFS, over the {TARGET_LUFS} it aims at"
        );
        assert!(
            reached > TARGET_LUFS - 6.0,
            "the piece came out at {reached:.1} LUFS, which is nowhere near the target"
        );
    }

    #[test]
    fn a_track_nobody_aimed_keeps_its_fader() {
        // Every track of a project that was not composed, which is what makes this command safe to
        // run on one: it normalises the loudness and does not re-mix somebody's work.
        let (mut session, _) = balanced();
        let id = session.project().tracks[0].id;
        if let Some(entry) = session.project.track_mut(id) {
            entry.mixer.target_lufs = None;
            entry.mixer.gain_db = -3.0;
        }
        let report = session.balance_levels().expect("the piece renders");
        let level = &report.tracks[0];
        assert_eq!(level.target_lufs, None, "the strip is aimed nowhere");
        assert!(
            (level.now_db - (-3.0 + report.lift_db)).abs() < 1.0e-4,
            "an unaimed fader was set to {:.2} dB rather than left at -3 and lifted by {:.2}",
            level.now_db,
            report.lift_db
        );
    }

    /// A program with velocity layers gets louder as it is struck harder.
    ///
    /// Ignored for the reason `calibration` above is — it needs the optional SoundFont, and
    /// whether that is installed is a fact about the machine:
    ///
    /// ```text
    /// cargo test -p auris-session --lib strikes_harder -- --ignored --nocapture
    /// ```
    ///
    /// It guards the fork in `vendor/rustysynth`. The published crate reads a font's modulator
    /// lists and throws them away, and MuseScore General's Grand Piano opens its filter with one:
    /// without it the piano fell *twenty decibels* between MIDI velocity 74 and 76, where a
    /// velocity layer changes. A dependency bump that quietly took the fork away would put it
    /// back, and nothing else here would notice — the piece would still compose, still balance,
    /// and still be wrong.
    #[test]
    #[ignore]
    fn strikes_harder() {
        use super::super::SessionOptions;
        use auris_core::time::Ticks;
        use auris_core::{Note, PresetRef};

        let mut session = Session::new(SessionOptions::headless().with_shipped_fonts(true))
            .expect("a session opens");
        let path = crate::library::installed(&crate::library::GENERAL_MIDI_FONT)
            .expect("this test needs MuseScore General; see `tools/fetch-soundfonts.sh`");
        let font = session.import_soundfont(&path).expect("a font imports");
        let track = session
            .add_default_instrument_track("piano")
            .expect("a track");
        session
            .set_track_preset(
                track,
                PresetRef {
                    font,
                    bank: 0,
                    patch: 0,
                },
            )
            .expect("the grand piano");
        let clip = session
            .add_midi_clip(track, "one note", Ticks::ZERO, Ticks::from_beats(4.0))
            .expect("a clip");

        let mut last: Option<(i32, f32)> = None;
        for midi in [60, 70, 74, 76, 80, 100, 106, 108, 120] {
            session.remove_notes(clip, &[0]).ok();
            let mut note = Note::new(60, Ticks::ZERO, Ticks::from_beats(2.0));
            // Undoing the sampler's own compensation, so the number below is the velocity the
            // synthesiser is actually handed and the layer boundaries can be named exactly.
            note.velocity = (midi as f32 / 127.0).powi(2);
            session.add_note(clip, note).expect("a note");
            let rendered = session
                .render_snapshot(session.project().clone())
                .expect("one note renders");
            let peak = analyze_loudness_cpu(&rendered).peak_db();
            if let Some((was_midi, was)) = last {
                assert!(
                    peak > was - 0.5,
                    "struck at {midi} the piano peaked at {peak:.1} dBFS, under the {was:.1} it \
                     reached at {was_midi} — the font's modulators are not being read"
                );
            }
            last = Some((midi, peak));
        }
    }
}
