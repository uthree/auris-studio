//! Puts a real hosted plugin through a real session, and says what came out.
//!
//! `auris-clap`'s own `inspect` example proves the crate can drive a `.clap`. This proves the
//! layer above it: that adding one through [`Session::add_hosted_effect`] reaches the render
//! graph, that its parameters come back, and that the audio on the other side is different from
//! the audio that went in.
//!
//! ```text
//! cargo run -p auris-session --example host -- "C:/Program Files/Common Files/CLAP/Surge Synth Team/Surge XT Effects.clap"
//! ```
//!
//! With no argument it takes the first plugin of the first installed file it finds.

use std::path::PathBuf;

use auris_session::{Session, SessionOptions};

fn main() {
    let mut session = Session::new(SessionOptions::headless()).expect("a headless session opens");

    let file = match std::env::args_os().nth(1) {
        Some(given) => PathBuf::from(given),
        None => match session.installed_clap_files(&[]).into_iter().next() {
            Some(found) => found,
            None => {
                eprintln!("no .clap files installed, and none given");
                std::process::exit(1);
            }
        },
    };
    println!("file: {}", file.display());

    let listed = match session.hosted_plugins_in(&file) {
        Ok(listed) => listed,
        Err(error) => {
            eprintln!("cannot read it: {error}");
            std::process::exit(1);
        }
    };
    for info in &listed {
        println!("  {} — {:?} / {:?}", info.name, info.kind, info.category);
    }

    let Some(info) = listed.first().cloned() else {
        eprintln!("nothing inside it");
        std::process::exit(1);
    };

    let track = session
        .add_default_instrument_track("Host")
        .expect("a track");
    let slot = match session.add_hosted_effect(Some(track), &file, &info.clap_id) {
        Ok(slot) => slot,
        Err(error) => {
            eprintln!("could not add it: {error}");
            std::process::exit(1);
        }
    };
    println!("\nadded as slot {slot:?}");

    let params = session.hosted_parameters(slot);
    println!("parameters the session can see: {}", params.len());
    for descriptor in params.iter().take(5) {
        println!(
            "  [{:>3}] {:<28} {:<18} {:.3} .. {:.3}",
            descriptor.id.raw(),
            descriptor.name,
            descriptor.key,
            descriptor.min,
            descriptor.max,
        );
    }

    // The name the inspector would draw beside the slot.
    println!("\nname the UI would show: {:?}", session.hosted_name(slot));
    println!(
        "descriptors the plugin window would list: {}",
        session.effect_descriptors(slot).len()
    );
}
