//! Loads a real `.clap` file and reports what this crate makes of it.
//!
//! The unit tests host a plugin compiled into the test binary, which proves the code paths but
//! not the assumptions — a plugin somebody else wrote will have opinions about parameter ids,
//! port layouts and what it does with a block of audio that a fixture never will. This is how a
//! real one gets pointed at the crate.
//!
//! ```text
//! cargo run -p auris-clap --example inspect -- "C:/Program Files/Common Files/CLAP/Surge Synth Team/Surge XT.clap"
//! ```
//!
//! With no arguments it walks the platform's CLAP folders and inspects everything it finds.

use std::path::{Path, PathBuf};

use auris_clap::{ClapLibrary, ClapPlugin};
use auris_core::buffer::AudioBuffer;
use auris_core::param::ParamId;
use auris_core::plugin::{Effect, Parameterized, PluginKind, PrepareContext, ProcessContext};

const SAMPLE_RATE: f64 = 48_000.0;
const BLOCK: usize = 512;

fn main() {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let files = match args.is_empty() {
        false => args,
        true => discover(),
    };

    if files.is_empty() {
        eprintln!("no .clap files given, and none found in the usual places");
        eprintln!("usage: cargo run -p auris-clap --example inspect -- <file.clap>");
        std::process::exit(1);
    }

    for file in files {
        inspect(&file);
    }
}

fn inspect(file: &Path) {
    println!("\n=== {} ===", file.display());

    // SAFETY: the person running this example asked for this file by name.
    let library = match unsafe { ClapLibrary::load(file) } {
        Ok(library) => library,
        Err(error) => {
            println!("  cannot load: {error}");
            return;
        }
    };

    let plugins = match library.plugins() {
        Ok(plugins) => plugins,
        Err(error) => {
            println!("  cannot read: {error}");
            return;
        }
    };

    println!("  {} plugin(s)", plugins.len());
    for info in &plugins {
        println!(
            "\n  {} — {} {} [{:?} / {:?}]",
            info.name, info.vendor, info.version, info.kind, info.category
        );
        println!("    auris id: {}", info.auris_id());

        let mut plugin = match library.instantiate(&info.clap_id) {
            Ok(plugin) => plugin,
            Err(error) => {
                println!("    cannot instantiate: {error}");
                continue;
            }
        };

        report_ports(&mut plugin);
        report_params(&mut plugin);
        report_state(&mut plugin);
        render(&mut plugin, info.kind);
    }
}

/// What ports the plugin wants, which is the thing a host cannot afford to assume.
///
/// A second input port is a sidechain, and it is not optional: a plugin that declares one indexes
/// it whether or not the host has anything to send there. Printing the layout is how a plugin that
/// is about to be hosted gets checked against the buffers it is going to be handed.
fn report_ports(plugin: &mut ClapPlugin) {
    let ports = plugin.ports(2);
    println!(
        "    ports: in {:?} (main {:?}), out {:?} (main {:?})",
        ports.inputs, ports.main_input, ports.outputs, ports.main_output
    );
}

fn report_params(plugin: &mut ClapPlugin) {
    let params = plugin.parameters().to_vec();
    println!("    {} parameter(s)", params.len());
    for descriptor in params.iter().take(8) {
        println!(
            "      [{:>3}] {:<28} {:<14} {:>10.3} .. {:<10.3} default {:.3}",
            descriptor.id.raw(),
            truncate(&descriptor.name, 28),
            descriptor.key,
            descriptor.min,
            descriptor.max,
            descriptor.default,
        );
    }
    if params.len() > 8 {
        println!("      … and {} more", params.len() - 8);
    }

    // The interesting question is not whether the values arrive but whether the *ids* survive:
    // a plugin numbering its parameters 0..n would hide an index/id confusion, one numbering
    // them anything else would expose it.
    let sequential = params
        .iter()
        .enumerate()
        .all(|(index, descriptor)| descriptor.key == format!("clap.{index}"));
    println!(
        "      parameter ids are {}",
        match sequential {
            true => "sequential from zero (would not catch an index/id mix-up)",
            false => "not the slice positions — a real test of the id mapping",
        }
    );
}

fn report_state(plugin: &mut ClapPlugin) {
    match plugin.save_state() {
        Ok(bytes) => println!("    state: {} bytes", bytes.len()),
        Err(error) => println!("    state: {error}"),
    }
}

fn render(plugin: &mut ClapPlugin, kind: PluginKind) {
    let ctx = PrepareContext::new(SAMPLE_RATE, BLOCK, 2);
    let mut effect = match plugin.activate(&ctx) {
        Ok(effect) => effect,
        Err(error) => {
            println!("    cannot activate: {error}");
            return;
        }
    };

    // A 440 Hz sine at -6 dBFS, which is something a filter or a reverb will visibly act on.
    let mut buffer = AudioBuffer::stereo(BLOCK, SAMPLE_RATE);
    for frame in 0..BLOCK {
        let sample =
            0.5 * (std::f32::consts::TAU * 440.0 * frame as f32 / SAMPLE_RATE as f32).sin();
        for channel in 0..buffer.channel_count() {
            buffer.channel_mut(channel)[frame] = sample;
        }
    }
    let before = peak(&buffer);

    let ctx = ProcessContext::realtime(SAMPLE_RATE, BLOCK, 0, 120.0, true);
    effect.process(&mut buffer, &ctx);
    let after = peak(&buffer);

    println!("    rendered one block: peak in {before:.4} → out {after:.4}");
    if kind == PluginKind::Instrument {
        println!("      (an instrument with no notes; silence out is the correct answer)");
    }

    // Move the first parameter to the far end of its range and see whether anything changes.
    // Not every parameter is audible on a sine, so a flat result is a shrug, not a failure.
    if let Some(descriptor) = plugin.parameters().first().cloned() {
        effect.set_param(ParamId(0), descriptor.max);
        let mut again = AudioBuffer::stereo(BLOCK, SAMPLE_RATE);
        for channel in 0..again.channel_count() {
            again
                .channel_mut(channel)
                .copy_from_slice(&vec![0.25; BLOCK]);
        }
        effect.process(&mut again, &ctx);
        println!(
            "    `{}` at {:.3}: peak {:.4}, and the plugin reports {:?}",
            descriptor.name,
            descriptor.max,
            peak(&again),
            plugin.value(ParamId(0)),
        );
    }

    plugin.deactivate(effect);
    println!("    deactivated cleanly");
}

fn peak(buffer: &AudioBuffer) -> f32 {
    (0..buffer.channel_count())
        .flat_map(|channel| buffer.channel(channel).iter())
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
}

fn truncate(text: &str, width: usize) -> String {
    match text.chars().count() > width {
        true => text.chars().take(width - 1).chain(['…']).collect(),
        false => text.to_string(),
    }
}

/// Where CLAP plugins live, per the specification's search paths.
fn discover() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if cfg!(target_os = "windows") {
        if let Ok(common) = std::env::var("CommonProgramFiles") {
            roots.push(PathBuf::from(common).join("CLAP"));
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            roots.push(PathBuf::from(local).join("Programs/Common/CLAP"));
        }
    } else {
        roots.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        if let Ok(home) = std::env::var("HOME") {
            roots.push(PathBuf::from(&home).join("Library/Audio/Plug-Ins/CLAP"));
            roots.push(PathBuf::from(&home).join(".clap"));
        }
        roots.push(PathBuf::from("/usr/lib/clap"));
    }

    let mut found = Vec::new();
    for root in roots {
        collect(&root, &mut found);
    }
    found
}

fn collect(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // On macOS a `.clap` is a bundle directory, so the extension is checked before the
        // file type, not after.
        if path
            .extension()
            .is_some_and(|extension| extension == "clap")
        {
            found.push(path);
        } else if path.is_dir() {
            collect(&path, found);
        }
    }
}
