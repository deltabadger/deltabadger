Deltabadger is under ongoing rewrite from Rails to Rust.

## Development

- TDD: when planning or adding a new feature, write tests first, and present them to review (as part of the plan)
- Before doing anything always ask yourself: what is the best/the smartest way to do it
- Always check if our stack doesn't have built in solution already
- Environment variables: see `.env.example`

## PRs

- write all comments and PRs in neutral informative language for users of open-source repository
- never include any information about closed infrastructure, users of the platform, or specific incidents

## Rust

- The Rust port lives in `rust/`. Rails is the oracle: Rust must match Rails' pages, answers and database effects, proven by parity tests against recorded Rails vectors (`script/rust/`). Run `cd rust && cargo nextest run` after Rust changes.
- Rust guards pin the Rails source files they port. A Rails change to a pinned file must re-record its vectors and update the Rust side in the same PR.

## Old stack

- Ruby 4 / Rails 8
- Node.js 18.19.1
- Hotwire (Turbo + Stimulus) for frontend
- SQLite with Solid Queue (background jobs), Solid Cache, Solid Cable (websockets)
- Tauri 2.x (Rust) for desktop app
- Docker deployment supported
- Sass (.sass)

## Rails
- Use Rails style guidelines: `.claude/rails.md`
- Run `bin/rails test` after every change
- Read `db/schema.rb` for the data model, and the model file itself for enums and associations — they are the source of truth
- After every change in dependencies or deployment look check Docker settings if they need updates

