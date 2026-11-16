# Development

## Requirements

- Rust (stable)
- [Bun](https://bun.sh)

## 1. Install dependencies

```bash
bun install
```

## 2. Start the app

```bash
bin/rust-dev
```

It builds the web assets, runs the app on http://127.0.0.1:3001 and rebuilds styles and scripts on save.

## Running tests

```bash
script/rust/build_assets.sh
cd rust && cargo nextest run
```
