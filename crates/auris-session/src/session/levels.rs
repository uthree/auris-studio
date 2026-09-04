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
//! [`MixerStrip::target_lufs`](auris_core::project::MixerStrip::target_lufs), written by the
//! composer because the composer is the only part of this program that knows what a track is for.
//! A strip with no target — anything a person made by hand — keeps its fader and travels with the
//! offset, so running this over an ordinary project is a loudness normalisation and nothing more.
//! That is the honest limit of it, and it is a limit about knowledge rather than about arithmetic.

use auris_core::{Project, TrackId};
use auris_dsp::integrated_lufs;
use auris_engine::EngineCommand;
use auris_engine::OfflineOptions;
use auris_gpu::analysis::analyze_loudness_cpu;

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

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
        self.begin_transaction(Edit::BalanceLevels);
        let balanced = self.balance_now();
        self.finish_balance(balanced)
    }

    /// Closes the balance transaction, restoring every fader when any later measurement failed.
    fn finish_balance(
        &mut self,
        balanced: Result<BalanceReport, SessionError>,
    ) -> Result<BalanceReport, SessionError> {
        match balanced.is_ok() {
            true => {
                self.end_transaction();
            }
            // Earlier faders may already have moved before a later stem or the full mix failed.
            // Put both the document and the live graph back, and leave no history step behind.
            false => {
                self.revert_transaction();
            }
        }
        balanced
    }

    /// The balance pass itself, with no step of its own in the history.
    ///
    /// Composing calls this rather than [`Self::balance_levels`] because the levels are part of
    /// the piece it just wrote: a person who takes back "composing a piece" wants the document
    /// they had before it, not the same piece with the faders where the composer first guessed
    /// them.
    pub(super) fn balance_now(&mut self) -> Result<BalanceReport, SessionError> {
        let levelled: Vec<TrackId> = self
            .project
            .tracks
            .iter()
            .filter(|track| !track.kind.is_bus())
            .map(|track| track.id)
            .collect();

        let mut tracks: Vec<TrackLevel> = Vec::new();
        for &id in &levelled {
            let measured = self.measure_alone(id)?;
            let entry = self
                .project
                .track(id)
                .expect("a track that was just listed");
            let was_db = entry.mixer.gain_db;
            let name = entry.name.clone();
            let target_lufs = entry.mixer.target_lufs;
            let now_db = match (target_lufs, measured) {
                (Some(target), Some(measured)) => fader_for(target, measured, was_db),
                // A track nothing identifies, and a track that made no sound, are both left where
                // they are. There is nothing to aim the first one at, and turning the second one
                // up would be setting a level from a measurement that does not exist.
                _ => was_db,
            };
            self.write_fader(id, now_db);
            tracks.push(TrackLevel {
                name,
                target_lufs,
                measured_lufs: measured,
                was_db,
                now_db,
            });
        }

        // The whole piece, once the parts sit where they should — and then the two ways there are
        // of making it louder, in the order that spends the cheaper one first.
        //
        // Raising every fader together drives the master's own limiter, which is what a master is
        // for and where the last few decibels of a dense mix come from. Raising the master fader
        // does not: it is *after* the chain, so nothing catches what it adds and it has to stop at
        // the ceiling. So the faders go up as far as the limiter's allowance and their own travel
        // permit, and the master picks up whatever is left in the headroom.
        let mix = self.render_snapshot(self.project.clone())?;
        let balanced_lufs = integrated_lufs(&mix);
        let headroom = tracks
            .iter()
            .map(|level| FADER_RANGE_DB.1 - level.now_db)
            .fold(f32::INFINITY, f32::min);
        let lift = balanced_lufs.map_or(0.0, |lufs| {
            faders_lift_db(lufs, analyze_loudness_cpu(&mix).true_peak_db(), headroom)
        });
        for (level, &id) in tracks.iter_mut().zip(&levelled) {
            level.now_db += lift;
            self.write_fader(id, level.now_db);
        }

        // Rendered again rather than predicted, because the limiter has just been given something
        // to do and only a render knows how much of the lift it kept.
        let lifted = self.render_snapshot(self.project.clone())?;
        let lifted_lufs = integrated_lufs(&lifted);
        let master_db = lifted_lufs.map_or(0.0, |lufs| {
            master_gain_db(lufs, analyze_loudness_cpu(&lifted).true_peak_db())
        });
        self.project.master.gain_db = master_db;
        self.send(EngineCommand::SetMasterGain(master_db));

        Ok(BalanceReport {
            tracks,
            lift_db: lift,
            master_db,
            balanced_lufs,
            // The master fader is a gain and the last thing in the signal path, so where the piece
            // ends up is where it measured plus where the fader went. This is the one number here
            // that is arithmetic rather than measurement, and it is exact.
            now_lufs: lifted_lufs.map(|lufs| lufs + master_db),
        })
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
    fn a_fader_moves_by_the_distance_the_measurement_is_out() {
        // A track worth -18 on its own, set to -12, measures -30. Aimed at -20 it wants six back.
        assert!((fader_for(-20.0, -30.0, -12.0) - -2.0).abs() < 1.0e-4);
        // And the fader cannot leave the range the mixer offers, however far out the measurement
        // is: a track that measured silence-adjacent would otherwise ask for a hundred decibels.
        assert_eq!(fader_for(-20.0, -120.0, 0.0), FADER_RANGE_DB.1);
        assert_eq!(fader_for(-60.0, 0.0, 0.0), FADER_RANGE_DB.0);
    }

    #[test]
    fn a_failed_balance_restores_faders_already_written() {
        use super::super::SessionOptions;

        let mut session = Session::new(SessionOptions::headless()).expect("a headless session");
        let track = session.project.add_audio_track("Part");
        session.begin_transaction(Edit::BalanceLevels);
        let before = session.project().track(track).unwrap().mixer.gain_db;
        session.write_fader(track, before + 6.0);

        let failed = session.finish_balance(Err(SessionError::UnknownTrack(u64::MAX)));

        assert!(failed.is_err());
        assert_eq!(
            session.project().track(track).unwrap().mixer.gain_db,
            before,
            "the partial fader move survived the error"
        );
        assert!(!session.can_undo(), "a failed balance left a history step");
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

    /// Prints what every preset measures, which is where the targets come from.
    ///
    /// Ignored, and it has to be: it needs the shipped SoundFont, and whether that is installed is
    /// a fact about the machine rather than about the code — the suite's own sessions run with no
    /// font at all so that a developer's laptop and a CI runner agree about what a document holds.
    /// Run it by hand, on a machine with the library, when a level constant is in question:
    ///
    /// ```text
    /// cargo test -p auris-session --lib calibration -- --ignored --nocapture
    /// ```
    ///
    /// What comes out is one row per part — where its fader is, what it measured alone, and where
    /// it was aiming — and one per piece. [`auris_compose::Role::target_lufs`] is the median of the
    /// first column across the eight presets, normalised back to each role's own default fader,
    /// and this is the only place that number can be checked. A role whose parts already measure
    /// what they are aiming at is a fixed point, which is the property that says the table is
    /// still the table.
    #[test]
    #[ignore]
    fn calibration() {
        use super::super::SessionOptions;
        for name in auris_compose::PRESETS.iter().map(|preset| preset.name) {
            let spec = auris_compose::preset(name)
                .expect("a shipped preset")
                .spec();
            // With the library, and *without* the balance pass: the table below is derived from
            // what the composer wrote, and measuring a piece whose faders have already been set by
            // measurement would only tell us that they had been.
            let mut session = Session::new(
                SessionOptions::headless()
                    .with_shipped_fonts(true)
                    .with_balance(false),
            )
            .expect("a session opens");
            session
                .compose(&auris_compose::compose(&spec))
                .expect("a preset composes");
            let started = std::time::Instant::now();
            let played: Vec<TrackId> = session
                .project()
                .tracks
                .iter()
                .filter(|track| !track.kind.is_bus())
                .map(|track| track.id)
                .collect();
            for id in played {
                let strip = session.project().track(id).expect("a listed track");
                let (part, fader, target) = (
                    strip.name.clone(),
                    strip.mixer.gain_db,
                    strip.mixer.target_lufs,
                );
                let measured = session.measure_alone(id).expect("a stem renders");
                println!(
                    "{name}\t{part}\t{fader:.1} dB\t{}\taiming {}",
                    measured.map_or("silent".into(), |lufs| format!("{lufs:.1} LUFS")),
                    target.map_or("nowhere".into(), |lufs| format!("{lufs:.1}")),
                );
            }
            let mix = session
                .render_snapshot(session.project().clone())
                .expect("the mix renders");
            println!(
                "{name}\tMIX\t\t{}\ttrue peak {:.1} dBFS\tmeasured in {:.1}s",
                integrated_lufs(&mix).map_or("silent".into(), |lufs| format!("{lufs:.1} LUFS")),
                analyze_loudness_cpu(&mix).true_peak_db(),
                started.elapsed().as_secs_f32()
            );
        }
    }

    /// Four bars of two parts on the built-in instruments, composed and balanced.
    ///
    /// No SoundFont, so it measures the same on every machine — and short, because every one of
    /// these renders the piece once per part and twice more.
    fn balanced() -> (Session, BalanceReport) {
        use super::super::SessionOptions;
        let spec = auris_compose::SongSpec::parse(
            r#"
            form = ["verse"]
            seed = 1
            [section.verse]
            bars = 4
            [[part]]
            name = "tune"
            role = "melody"
            [[part]]
            name = "low"
            role = "bass"
            "#,
        )
        .expect("a specification this file wrote");
        let mut session = Session::new(SessionOptions::headless().with_balance(false))
            .expect("a headless session opens");
        session
            .compose(&auris_compose::compose(&spec))
            .expect("two parts compose");
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
            let target = level.target_lufs.expect("a composed part is aimed") + report.lift_db;
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
    /// Ignored for the reason `calibration` above is — it needs the shipped SoundFont, and
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

        let mut session = Session::new(
            SessionOptions::headless()
                .with_shipped_fonts(true)
                .with_balance(false),
        )
        .expect("a session opens");
        session.install_shipped_fonts();
        let Some(font) = session.project().soundfonts.keys().next().copied() else {
            panic!("this test needs the shipped library; see `tools/fetch-soundfonts.sh`");
        };
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
