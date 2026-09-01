# Development

## Requirements

- Ruby 3.4.8
- Node.js 18.19.1

Use [asdf](https://asdf-vm.com) or your preferred version manager.

## 1. Install dependencies

```bash
bin/setup
```

## 2. Database

```bash
bundle exec rails db:prepare
```

## 3. Start the app

```bash
bin/dev
```

This starts the Rails server with Solid Queue (background jobs) running in-process via Puma.

Alternatively, run services separately:

Terminal 1 — Rails (with background jobs):

```bash
rails s
```

Terminal 2 — JavaScript bundler (optional, for live reloading):

```bash
npm run build:watch
```

## Running tests

```bash
bin/rails test
```
