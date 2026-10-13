// =============================================================================
// build.rs - embeds the application icon, the Windows manifest and version info.
//
// This goes through the `winresource` crate, which locates the right resource
// compiler for the active toolchain: `rc.exe` from the MSVC build tools, or
// `windres` from MinGW on the GNU toolchain.
//
// It is worth using rather than shelling out by hand: an earlier version
// invoked `rc` directly, and when the compiler was not on PATH the step
// silently did nothing, producing an executable with no icon and no manifest.
// `expect` here makes that class of mistake impossible to ship again.
// =============================================================================

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-changed=build.rs");

    let mut res = winresource::WindowsResource::new();

    res.set_icon("assets/app.ico");
    res.set_manifest_file("assets/app.manifest");

    // Shown in the file properties and in Task Manager.
    res.set("FileDescription", "XMG Focus Optimizer");
    res.set("ProductName", "XMG Focus Optimizer");
    res.set("CompanyName", "XMG Focus Optimizer contributors");
    res.set("LegalCopyright", "MIT licensed");
    res.set("FileVersion", env!("CARGO_PKG_VERSION"));
    res.set("ProductVersion", env!("CARGO_PKG_VERSION"));

    if let Err(e) = res.compile() {
        panic!(
            "could not embed the icon/manifest resources: {e}\n\
             The executable needs these for its taskbar and Start-menu identity.\n\
             On MSVC make sure the Visual Studio C++ build tools are installed."
        );
    }
}