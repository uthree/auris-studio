//! Embeds the application icon in Windows executables.

fn main() {
    println!("cargo:rerun-if-changed=assets/windows.rc");
    println!("cargo:rerun-if-changed=assets/auris-studio.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // gpui loads icon resource 1 for the window class, so the executable and its
        // running windows share the same icon without a separate runtime asset path.
        embed_resource::compile_for("assets/windows.rc", ["auris-studio"], embed_resource::NONE)
            .manifest_required()
            .expect("could not embed the Auris Studio application icon");
    }
}
