//! The document's tuning: choosing one, and reading one from Scala files.
//!
//! A tuning reaches every pitched track at once and is heard through the render graph, so each
//! change here is one undo step and a rebuild — the same cost as a tempo change, and for the
//! same reason: every note-on the graph holds is about to sound somewhere else.

use std::path::Path;

use auris_core::{Tuning, TuningPreset};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

/// The largest Scala file read, in bytes.
///
/// [`MAX_DEGREES`](auris_core::tuning::MAX_DEGREES) lines of cents with a comment each come to
/// well under this; anything bigger is not a tuning, and reading it whole would be the mistake.
const MAX_SCALA_BYTES: u64 = 1 << 20;

impl Session {
    /// The tuning every pitched track plays in.
    pub fn tuning(&self) -> &Tuning {
        &self.project.tuning
    }

    /// Replaces the document's tuning.
    ///
    /// The same tuning again is no step at all, so choosing what is already chosen leaves the undo
    /// history alone.
    pub fn set_tuning(&mut self, tuning: Tuning) {
        if tuning == self.project.tuning {
            return;
        }
        self.record(Edit::SetTuning);
        self.project.tuning = tuning;
        self.invalidate_graph();
    }

    /// Replaces the document's tuning with a preset.
    pub fn set_tuning_preset(&mut self, preset: TuningPreset) -> Result<(), SessionError> {
        let tuning = preset.tuning()?;
        self.set_tuning(tuning);
        Ok(())
    }

    /// Moves the whole tuning so that `key` sounds at `hz`, every interval kept.
    ///
    /// Concert pitch: `set_tuning_reference(69, 442.0)` in the standard tuning is A = 442.
    pub fn set_tuning_reference(&mut self, key: u8, hz: f64) -> Result<(), SessionError> {
        let tuning = self.project.tuning.with_reference(key, hz)?;
        self.set_tuning(tuning);
        Ok(())
    }

    /// Reads a Scala `.scl` file, placed on the keyboard by a `.kbm` file when one is given, and
    /// makes it the document's tuning.
    ///
    /// The tuning is copied into the document rather than referred to, unlike a SoundFont: it is a
    /// few hundred bytes, and a song that sounded different on a machine missing the file would be
    /// a worse surprise than a silent track.
    pub fn import_scala(
        &mut self,
        scale: &Path,
        keyboard: Option<&Path>,
    ) -> Result<(), SessionError> {
        let mut tuning = Tuning::from_scala(&read_scala(scale)?)?;
        if let Some(keyboard) = keyboard {
            tuning = tuning.with_keyboard_map(&read_scala(keyboard)?)?;
        }
        self.set_tuning(tuning);
        Ok(())
    }
}

/// A Scala file's text, refused before it is read when it is too big to be one.
fn read_scala(path: &Path) -> Result<String, SessionError> {
    let bytes = std::fs::metadata(path)
        .map_err(|error| auris_io::IoError::from_fs(path, error))?
        .len();
    if bytes > MAX_SCALA_BYTES {
        return Err(SessionError::TuningFileTooLarge {
            path: path.to_path_buf(),
            bytes,
        });
    }
    let raw = std::fs::read(path).map_err(|error| auris_io::IoError::from_fs(path, error))?;
    // Scala files predate UTF-8 and the archive's descriptions are often Latin-1; a name with a
    // replacement character in it is a better outcome than a file that will not open.
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;
    use auris_core::theory::pitch::PitchClass;

    #[test]
    fn choosing_a_tuning_is_one_undo_step_and_choosing_it_again_is_none() {
        let mut session = session();
        session.forget_history();
        session.set_tuning_preset(TuningPreset::Edo(31)).unwrap();
        session.set_tuning_preset(TuningPreset::Edo(31)).unwrap();
        assert_eq!(session.tuning().steps_per_period(), 31);
        assert_eq!(session.undo(), Some(Edit::SetTuning));
        assert!(session.tuning().is_standard());
        assert_eq!(session.undo(), None);
    }

    /// Power at one frequency, as one DFT bin.
    fn power_at(samples: &[f32], rate: f64, hz: f64) -> f64 {
        let step = std::f64::consts::TAU * hz / rate;
        let (re, im) = samples
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(re, im), (n, sample)| {
                let phase = step * n as f64;
                (
                    re + f64::from(*sample) * phase.cos(),
                    im - f64::from(*sample) * phase.sin(),
                )
            });
        re * re + im * im
    }

    #[test]
    fn a_retuned_song_is_heard_where_the_tuning_puts_it() {
        // One key a step above middle C, played by the chiptune voice and exported. In 31-tone
        // equal temperament that is 267.5 Hz — not the 277.2 of C sharp, and not middle C.
        let mut session = session();
        let track = session
            .project
            .add_instrument_track("Lead", auris_synth::Chiptune::ID);
        let clip = session
            .add_midi_clip(
                track,
                "Step",
                auris_core::Ticks::ZERO,
                auris_core::Ticks::from_beats(4.0),
            )
            .unwrap();
        session
            .add_note(
                clip,
                auris_core::Note::new(
                    61,
                    auris_core::Ticks::ZERO,
                    auris_core::Ticks::from_beats(4.0),
                ),
            )
            .unwrap();
        session.set_tuning_preset(TuningPreset::Edo(31)).unwrap();
        let tuned = session.tuning().hz(61);
        assert!((tuned - 267.54).abs() < 0.01, "{tuned}");

        let mix = session
            .render_job()
            .render(
                &auris_engine::OfflineOptions::whole_project(),
                &mut auris_engine::RenderProgress::default(),
            )
            .unwrap();
        let rate = mix.sample_rate();
        // The second quarter of a second onwards, past the attack.
        let steady = &mix.channel(0)[(rate * 0.25) as usize..(rate * 1.25) as usize];
        let at_tuned = power_at(steady, rate, tuned);
        for elsewhere in [auris_core::tuning::MIDDLE_C_HZ, 277.18] {
            let there = power_at(steady, rate, elsewhere);
            assert!(
                at_tuned > 20.0 * there,
                "{tuned:.1} Hz: {at_tuned:.3e}, {elsewhere:.1} Hz: {there:.3e}"
            );
        }
    }

    #[test]
    fn concert_pitch_keeps_the_scale_and_moves_the_reference() {
        let mut session = session();
        session
            .set_tuning_preset(TuningPreset::Just(PitchClass::new(9)))
            .unwrap();
        session.set_tuning_reference(69, 432.0).unwrap();
        assert_eq!(session.tuning().name(), "Just intonation on A");
        assert!((session.tuning().hz(69) - 432.0).abs() < 1e-9);
        assert!(session.set_tuning_reference(69, -1.0).is_err());
        assert!(
            (session.tuning().hz(69) - 432.0).abs() < 1e-9,
            "a refusal changes nothing"
        );
    }

    #[test]
    fn a_scala_file_and_its_keyboard_map_become_the_tuning() {
        let directory = tempfile::tempdir().unwrap();
        let scale = directory.path().join("slendro.scl");
        std::fs::write(
            &scale,
            "! slendro\nSlendro\n5\n240.0\n480.0\n720.0\n960.0\n2/1\n",
        )
        .unwrap();
        let keyboard = directory.path().join("slendro.kbm");
        std::fs::write(&keyboard, "0\n0\n127\n62\n62\n293.66\n5\n").unwrap();

        let mut session = session();
        session.import_scala(&scale, Some(&keyboard)).unwrap();
        let tuning = session.tuning();
        assert_eq!(tuning.name(), "Slendro");
        assert_eq!(tuning.root_key(), 62);
        assert!((tuning.hz(67) - 2.0 * 293.66).abs() < 1e-9);

        let missing = directory.path().join("missing.scl");
        assert!(session.import_scala(&missing, None).is_err());
        assert_eq!(session.tuning().name(), "Slendro");
    }
}
