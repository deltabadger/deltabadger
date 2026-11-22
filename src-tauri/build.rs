fn main() {
  // The tests find the sidecar Tauri bundles (binaries/deltabadger-engine-<target triple>) by this.
  println!("cargo:rustc-env=DESKTOP_TARGET_TRIPLE={}", std::env::var("TARGET").unwrap());
  copy_fonts();
  // The app's own commands, so capabilities/default.json can grant them to the engine's pages: the window shows
  // http://127.0.0.1, a remote origin, and Tauri refuses a remote page every app command no capability names.
  // test_report exists only in debug builds (the proofs' reports).
  tauri_build::try_build(
    tauri_build::Attributes::new()
      .app_manifest(tauri_build::AppManifest::new().commands(&["update_state", "check_for_updates", "install_update", "retry_update", "sign_in_window", "test_report"])),
  )
  .expect("failed to run tauri-build")
}

/// D-LANGUAGE-STEP: the status page's language picker draws in the app's own faces: Montserrat, and the Greek faces the
/// app serves for what Montserrat lacks. Copied next to ui/index.html (git-ignored) from the engine's sources, before
/// tauri-build bundles ui/; a file whose bytes are already there is left alone.
fn copy_fonts() {
  let ui = std::path::Path::new("ui");
  for source in ["../rust/web/fonts/Montserrat-VariableFont_wght.woff2", "../rust/web/public/fonts/GoogleSans-greek.woff2", "../rust/web/public/fonts/GoogleSans-greek-ext.woff2"] {
    println!("cargo:rerun-if-changed={source}");
    let bytes = std::fs::read(source).unwrap_or_else(|e| panic!("{source}: {e}"));
    let target = ui.join(std::path::Path::new(source).file_name().unwrap());
    if std::fs::read(&target).ok().as_deref() != Some(&bytes[..]) {
      std::fs::write(&target, &bytes).unwrap_or_else(|e| panic!("{}: {e}", target.display()));
    }
  }
}
