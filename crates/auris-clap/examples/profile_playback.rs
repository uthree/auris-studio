//! Measures a selected CLAP instrument with and without concurrent parameter-panel reads.
//! Run with `--release -- <file.clap> [block-frames] [seconds] [copies|mixed]` on an idle machine.

use auris_clap::ClapLibrary;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamId, PluginKind, PrepareContext, ProcessContext,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let file = args.next().ok_or("expected a CLAP file path")?;
    let frames: usize = args.next().map_or(Ok(512), |s| s.parse())?;
    let seconds: f64 = args.next().map_or(Ok(3.0), |s| s.parse())?;
    let selection = args.next().unwrap_or_else(|| "1".into());
    let mixed = selection == "mixed";
    let copies: usize = if mixed { 1 } else { selection.parse()? };
    if frames == 0 || !seconds.is_finite() || seconds <= 0.0 || !(1..=16).contains(&copies) {
        return Err("invalid duration or block size".into());
    }
    // SAFETY: the operator explicitly selected this third-party binary.
    let library = unsafe { ClapLibrary::load(&file) }?;
    let infos: Vec<_> = library
        .plugins()?
        .into_iter()
        .filter(|info| info.kind == PluginKind::Instrument)
        .collect();
    let groups = if mixed {
        vec![infos]
    } else {
        infos.into_iter().map(|info| vec![info; copies]).collect()
    };
    for group in groups {
        let label = group
            .iter()
            .map(|info| info.name.as_str())
            .collect::<Vec<_>>()
            .join(" + ");
        for mode in ["closed", "direct", "cached"] {
            let mut plugins = Vec::new();
            let mut instruments = Vec::new();
            let mut count = 0;
            for info in &group {
                let mut plugin = library.instantiate(&info.clap_id)?;
                count += plugin.parameters().len();
                instruments
                    .push(plugin.activate_instrument(&PrepareContext::new(48_000.0, frames, 2))?);
                plugins.push(plugin);
            }
            let finished = Arc::new(AtomicBool::new(false));
            let done = finished.clone();
            let mut panel_times = Vec::new();
            let (instruments, mut times, silent, energy, checksum) = std::thread::scope(|scope| {
                let audio = scope.spawn(move || {
                    let mut out = AudioBuffer::stereo(frames, 48_000.0);
                    let blocks = (seconds * 48_000.0 / frames as f64).ceil() as usize;
                    let mut times = Vec::with_capacity(blocks);
                    let mut silent = 0;
                    let mut energy = 0.0f64;
                    let mut checksum = 0xcbf29ce484222325u64;
                    let start = Instant::now();
                    for block in 0..blocks {
                        let notes: Vec<_> = if block == 0 {
                            [48, 55, 60, 64]
                                .map(|pitch| NoteEvent::note_on(0, pitch, 0.7))
                                .into()
                        } else {
                            Vec::new()
                        };
                        let context = ProcessContext::realtime(
                            48_000.0,
                            frames,
                            (block * frames) as u64,
                            120.0,
                            true,
                        );
                        let mut process_ms = 0.0;
                        let mut block_energy = 0.0;
                        let mut all_silent = true;
                        for instrument in &mut instruments {
                            let began = Instant::now();
                            instrument.process(&notes, &mut out, &context);
                            process_ms += began.elapsed().as_secs_f64() * 1000.0;
                            all_silent &= out.peak() == 0.0;
                            for sample in out.iter_channels().flatten() {
                                checksum = (checksum ^ u64::from(sample.to_bits()))
                                    .wrapping_mul(0x100000001b3);
                            }
                            block_energy += out
                                .iter_channels()
                                .flatten()
                                .map(|x| f64::from(*x).powi(2))
                                .sum::<f64>();
                        }
                        times.push(process_ms);
                        if block > 1 && all_silent {
                            silent += 1;
                        }
                        energy += block_energy;
                        let due = start
                            + Duration::from_secs_f64(((block + 1) * frames) as f64 / 48_000.0);
                        if let Some(wait) = due.checked_duration_since(Instant::now()) {
                            std::thread::sleep(wait);
                        }
                    }
                    done.store(true, Ordering::Release);
                    (instruments, times, silent, energy, checksum)
                });
                while !finished.load(Ordering::Acquire) {
                    if mode != "closed" {
                        let began = Instant::now();
                        for plugin in &mut plugins {
                            for index in 0..plugin.parameters().len() {
                                let id = ParamId(index as u32);
                                std::hint::black_box(if mode == "cached" {
                                    plugin.cached_value(id)
                                } else {
                                    plugin.value(id)
                                });
                            }
                        }
                        panel_times.push(began.elapsed().as_secs_f64() * 1000.0);
                    }
                    std::thread::sleep(Duration::from_millis(16));
                }
                audio
                    .join()
                    .map_err(|_| "audio measurement thread panicked")
            })?;
            for (plugin, instrument) in plugins.iter_mut().zip(instruments) {
                plugin.deactivate_instrument(instrument);
            }
            times.sort_by(f64::total_cmp);
            panel_times.sort_by(f64::total_cmp);
            let percentile = |values: &[f64], p: f64| {
                values
                    .get(((values.len().saturating_sub(1)) as f64 * p) as usize)
                    .copied()
                    .unwrap_or(0.0)
            };
            let deadline = frames as f64 / 48.0;
            println!(
                "{} frames={frames} mode={mode} params={count} block_p99_ms={:.3} block_max_ms={:.3} overruns={} panel_p99_ms={:.3} panel_max_ms={:.3} silent_blocks={silent} energy={energy:.6} checksum={checksum:016x}",
                label,
                percentile(&times, 0.99),
                percentile(&times, 1.0),
                times.iter().filter(|t| **t > deadline).count(),
                percentile(&panel_times, 0.99),
                percentile(&panel_times, 1.0)
            );
        }
    }
    Ok(())
}
