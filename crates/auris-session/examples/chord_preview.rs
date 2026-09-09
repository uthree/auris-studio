//! Renders the same block-chord audition used by the song sheet, without an audio device.

use auris_session::{ChordPreviewJob, prelude::SongSpec};
use std::{error::Error, path::Path, sync::atomic::AtomicBool};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: chord_preview SPEC.asong SECTION OUTPUT.wav".into());
    }
    let spec = SongSpec::parse(&std::fs::read_to_string(&args[0])?).map_err(|errors| {
        std::io::Error::other(
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    let preview = ChordPreviewJob::new(spec, args[1].clone(), None, 48_000.0)
        .run(&AtomicBool::new(false))
        .map_err(std::io::Error::other)?;
    auris_io::write_wav(Path::new(&args[2]), &preview.buffer, &Default::default())?;
    println!("{} bars: {}", preview.bars, preview.chord_names.join(" | "));
    println!(
        "{} seconds, fixed keyboard, -12 dBFS peak",
        preview.buffer.duration_seconds()
    );
    Ok(())
}
