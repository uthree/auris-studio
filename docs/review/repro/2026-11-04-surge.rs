//! Reproduce repeated graph rebuilds with a real Surge XT CLAP instrument.
//!
//! This is intentionally a small, headless probe for Windows plugin-hosting reviews. It creates
//! four tracks using the same plugin and adds twelve empty MIDI clips, timing each edit. The
//! final render is only a smoke check; the useful signal is whether graph rebuilds remain stable
//! and whether their latency grows without bound.

use auris_core::plugin::PluginKind;
use auris_core::{Note, Ticks};
use auris_session::{Session, SessionOptions};
use std::{error::Error, path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let file = args.first().map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(r"C:\Program Files\Common Files\CLAP\Surge Synth Team\Surge XT.clap")
    });

    let mut session = Session::new(
        SessionOptions::headless()
            .with_plugin_paths(vec![file.parent().unwrap_or(file.as_path()).to_path_buf()]),
    )?;
    let plugins = session.hosted_plugins_in(&file)?;
    let instrument = plugins
        .iter()
        .find(|plugin| plugin.kind == PluginKind::Instrument)
        .ok_or_else(|| format!("no instrument plugin found in {}", file.display()))?;
    println!(
        "plugin={} version={} vendor={} id={}",
        instrument.name, instrument.version, instrument.vendor, instrument.clap_id
    );

    let mut tracks = Vec::new();
    for index in 0..4 {
        let track = session.add_default_instrument_track(format!("Surge {index}"))?;
        session.set_hosted_instrument(track, &file, &instrument.clap_id)?;
        tracks.push(track);
    }

    let mut elapsed = Vec::new();
    let mut last_clip = None;
    for index in 0..12 {
        let started = Instant::now();
        let track = tracks[index % tracks.len()];
        last_clip = Some(session.add_midi_clip(
            track,
            format!("Stress {index}"),
            Ticks::from_beats((index / 4) as f64 * 4.0),
            Ticks::from_beats(4.0),
        )?);
        elapsed.push(started.elapsed());
        println!(
            "edit={index} elapsed_ms={:.3}",
            elapsed.last().unwrap().as_secs_f64() * 1000.0
        );
    }

    println!(
        "add_midi_clip_ms={}",
        elapsed
            .iter()
            .map(|duration| format!("{:.3}", duration.as_secs_f64() * 1000.0))
            .collect::<Vec<_>>()
            .join(",")
    );
    session.add_note(
        last_clip.unwrap(),
        Note::new(60, Ticks::ZERO, Ticks::from_beats(1.0)),
    )?;
    let rendered = session.render_job().render(
        &auris_engine::OfflineOptions::whole_project(),
        &mut auris_engine::RenderProgress::default(),
    )?;
    println!(
        "render_frames={} peak={:.6} finite={}",
        rendered.frame_count(),
        rendered.peak(),
        rendered
            .channels()
            .iter()
            .flatten()
            .all(|sample| sample.is_finite())
    );
    Ok(())
}
