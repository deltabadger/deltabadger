fn main() {
  // The tests find the sidecar Tauri bundles (binaries/deltabadger-engine-<target triple>) by this.
  println!("cargo:rustc-env=DESKTOP_TARGET_TRIPLE={}", std::env::var("TARGET").unwrap());
  tauri_build::build()
}
