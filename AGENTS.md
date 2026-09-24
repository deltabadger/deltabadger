Deltabadger is an open-source DCA bot, portfolio-rebalancing tool, and portfolio tracker for stocks and crypto developed in the TTD methodology. Tests are always written and presented to review first.

## Stack

- Ruby 3.4.8 / Rails 8.1
- Node.js 18.19.1
- Hotwire (Turbo + Stimulus) for frontend
- SQLite with Solid Queue (background jobs), Solid Cache, Solid Cable (websockets)
- Tauri 2.x (Rust) for desktop app
- Docker deployment supported
- Sass (.sass)

## Development

- TDD: when planning or adding a new feature, write tests first, and present them to review (as part of the plan)
- Before doing anything always ask yourself: what is the best/the smartest way to do it
- Always check if our stack doesn't have built in solution already
- Use Rails style guidelines: `.claude/rails.md`
- Environment variables: see `.env.example`
- Run `bin/rails test` after every change
- Read `db/schema.rb` for the data model, and the model file itself for enums and associations — they are the source of truth
- After every change in dependencies or deployment look check Docker settings if they need updates

## PRs

- write all comments and PRs in neutral informative language for users of open-source repository
- never include any information about closed infrastructure, users of the platform, or specific incidents