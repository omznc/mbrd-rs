//! Two small facts the binary cannot work out for itself at runtime.
//!
//! Both are here rather than in the code because both are properties of *this
//! build* rather than of the machine it eventually runs on, and a binary that
//! guesses either of them guesses wrong exactly when it matters: a
//! cross-compiled build asked at runtime what platform it is on gets the
//! honest answer for the wrong question.

fn main() {
    // The exact target triple, which is the key the update manifest is indexed
    // by. `std::env::consts::{ARCH, OS}` could be stitched into something
    // similar at runtime, but "similar" is the problem — it cannot tell
    // `x86_64-pc-windows-msvc` from `x86_64-pc-windows-gnu`, and those are two
    // different executables that must not be offered each other's updates.
    println!(
        "cargo:rustc-env=MBRD_TARGET={}",
        std::env::var("TARGET").expect("cargo always sets TARGET")
    );

    // The public key updates are verified against, baked in at build time and
    // absent by default. A build without it cannot install anything — see
    // `update::key` — which is the correct default for anybody building this
    // themselves: they have not published a manifest and are not signing one,
    // so the honest behaviour is an app that never tries.
    println!("cargo:rerun-if-env-changed=MBRD_UPDATE_KEY");
    println!("cargo:rerun-if-env-changed=MBRD_PACKAGED");
    if std::env::var_os("MBRD_PACKAGED").is_some() {
        println!("cargo:rustc-env=MBRD_PACKAGED=1");
    }

    // The icon, on Windows only. Cargo runs this script on the *host*, so the
    // question has to be asked of `CARGO_CFG_TARGET_OS` rather than of
    // `cfg!(windows)` — otherwise a Windows build from a Linux host silently
    // ships without one.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=../../packaging/windows/mbrd.rc");
        println!("cargo:rerun-if-changed=../../packaging/icons/mbrd.ico");
        embed_resource::compile("../../packaging/windows/mbrd.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("the icon resource should compile");
    }
}
