//! Embeds the icon and version info into the Windows executable, so Explorer
//! and the taskbar show the game's own icon instead of a blank one.
//!
//! Checks the *target* OS: this script runs on the build host, which for a
//! cross-build from Linux is not Windows.

fn main() {
    println!("cargo:rerun-if-changed=windows.rc");
    println!("cargo:rerun-if-changed=assets/bomberman.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("windows.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("embedding the Windows icon");
    }
}
