Deltabadger 3.0 is a Rust program. Deltabadger 2.x is the program before it: 3.0 reads and upgrades 2.x installs.

## Development

- TDD: when planning or adding a new feature, write tests first, and present them to review (as part of the plan)
- Before doing anything always ask yourself: what is the best/the smartest way to do it
- Always check if our stack doesn't have built in solution already
- Environment variables: see `.env.example`
- After every change in dependencies or deployment, check whether `Dockerfile` and the compose files need updates

## Layout

- The engine (server, CLI, bots) is the crate in `rust/`; the desktop app is `src-tauri/`
- Web UI sources: `rust/web/` (`javascript/`, `stylesheets/`, `fonts/`, `images/`, `public/`); `rust/build.rs` embeds the built files
- Translations: `rust/locales/`
- Build gates: `rust/gates/`
- Data model: `rust/schema/baseline.sql` and `rust/migrations/`

## Tests

- A fresh worktree runs `script/rust/build_assets.sh` once before building or testing (the crate embeds its output)
- After a change, run clippy and the test files it touches (`cd rust && cargo nextest run --test <file>`); the pull request runs the full suite, the merge queue every platform
- Recorded answers in `rust/tests/recorded/` are fixed data: never edit or re-record them
- An intended output change rewrites its expected file with `EXPECTED_REWRITE=1`; the page, tracker and decision grids have no rewrite
- 2.x interop tests run on the 2.70.21 image, in rust.yml's image job (`rust/.config/nextest.toml` `[profile.image]`)
- Never build two worktrees of the crate against one shared `CARGO_TARGET_DIR` at once: the build script's directory has the same hash in both, and one runs the other's `build.rs`
- Docs links: `python3 script/rust/doc_links.py`

## PRs

- write all comments and PRs in neutral informative language for users of open-source repository
- never include any information about closed infrastructure, users of the platform, or specific incidents
