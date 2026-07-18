//! Embed a Windows application manifest so Win32 dialogs use Common
//! Controls v6 visual styles (modern themed buttons/edits/combos).

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("app.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());

    let mut res = winresource::WindowsResource::new();
    res.set_manifest_file(manifest.to_str().expect("manifest path is valid UTF-8"));
    if let Err(error) = res.compile() {
        // Non-fatal: dialog code also activates ComCtl v6 at runtime.
        println!("cargo:warning=failed to embed Windows manifest: {error}");
    }
}
