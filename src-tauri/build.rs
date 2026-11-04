fn main() {
  // The tests find the sidecar Tauri bundles (binaries/deltabadger-engine-<target triple>) by this.
  println!("cargo:rustc-env=DESKTOP_TARGET_TRIPLE={}", std::env::var("TARGET").unwrap());
  // The app's own commands, so capabilities/default.json can grant them to the engine's pages: the window shows
  // http://127.0.0.1, a remote origin, and Tauri refuses a remote page every app command no capability names.
  // test_report exists only in debug builds (the proofs' reports).
  tauri_build::try_build(
    tauri_build::Attributes::new()
      .app_manifest(tauri_build::AppManifest::new().commands(&["check_for_updates", "test_report"])),
  )
  .expect("failed to run tauri-build")
}
