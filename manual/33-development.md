# Development

## Requirements

- Rust 1.94 or newer
- [Bun](https://bun.sh)
- [cargo-nextest](https://nexte.st) for the tests

## 1. Install dependencies

```bash
bun install
```

## 2. Start the app

```bash
bin/rust-dev
```

It builds the web assets and the engine, runs the app on http://127.0.0.1:3001 and rebuilds styles and scripts on save; reload the page to see them. A change to a template or to Rust code needs a restart. The install lives in tmp/rust-dev/ and survives restarts; delete the folder for a fresh one. `PORT` and `STORAGE_DIR` change the port and the folder.

## 3. The desktop app

```bash
bin/tauri-dev
```

It builds a debug engine as the app's sidecar (`script/rust/desktop_engine.sh`) and opens the app under a development identifier, so it never meets an installed Deltabadger.

## Building

The engine embeds the built web assets, so build them before `cargo build`:

```bash
script/rust/build_assets.sh
cd rust && cargo build --bin deltabadger
```

## Running tests

```bash
script/rust/build_assets.sh
cd rust && cargo nextest run
```
