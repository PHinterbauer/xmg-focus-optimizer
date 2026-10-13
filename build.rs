// =============================================================================
// build.rs - embeds the application icon and the Windows application manifest.
//
// Two resource compilers are supported so the project builds both with the
// GNU toolchain (windres ships with MinGW-w64) and with the MSVC toolchain
// (rc.exe from the Windows SDK). If neither is available the build still
// succeeds - the app simply falls back to a runtime-drawn icon.
// =============================================================================

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));

    let icon = manifest_dir.join("assets").join("app.ico");
    let manifest = manifest_dir.join("assets").join("app.manifest");

    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed=build.rs");

    // Windows resource IDs used below.
    const ID_ICON: u32 = 1;
    const ID_MANIFEST: u32 = 24; // RT_MANIFEST

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    // ------------------------------------------------------------- RC source
    let rc = out_dir.join("xmg_resources.rc");
    let mut body = String::new();
    if icon.exists() {
        body.push_str(&format!("{} ICON \"{}\"\n", ID_ICON, escape_win(&icon)));
    }
    if manifest.exists() {
        body.push_str(&format!(
            "{} {} \"{}\"\n",
            ID_ICON,
            ID_MANIFEST,
            escape_win(&manifest)
        ));
    }
    if body.is_empty() {
        return;
    }
    if std::fs::write(&rc, body).is_err() {
        return;
    }

    // ------------------------------------------------------ GNU: windres + ar
    if target_env == "gnu" {
        let obj = out_dir.join("xmg_resources.o");
        if run("windres", &["-i", rc.to_str().unwrap(), "-O", "coff", "-o", obj.to_str().unwrap()]) {
            let archive = out_dir.join("libxmg_resources.a");
            if run("ar", &["crs", archive.to_str().unwrap(), obj.to_str().unwrap()]) {
                println!("cargo:rustc-link-search=native={}", out_dir.display());
                println!("cargo:rustc-link-lib=static=xmg_resources");
                println!("cargo:rerun-if-env-changed=PATH");
            }
        }
        return;
    }

    // ---------------------------------------------------- MSVC: rc.exe + lib
    if target_env == "msvc" {
        let res = out_dir.join("xmg_resources.res");
        if run("rc", &["/nologo", "/fo", res.to_str().unwrap(), rc.to_str().unwrap()]) {
            println!("cargo:rustc-link-search=native={}", out_dir.display());
            println!("cargo:rustc-link-lib=static=xmg_resources");
            // Hand the .res straight to link.exe - MSVC has no ar.
            println!("cargo:rustc-link-arg-bins={}", res.display());
        }
    }
}

/// Escapes a path for use inside a quoted .rc string literal.
fn escape_win(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "\\\\")
}

/// Runs a tool, reporting (but not failing on) a non-zero exit.
fn run(program: &str, args: &[&str]) -> bool {
    match Command::new(program).args(args).status() {
        Ok(st) => st.success(),
        Err(_) => false,
    }
}