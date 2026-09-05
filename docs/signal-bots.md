# Signal bots are incomplete

**Status: the webhook receiver does not exist.** A signal bot can be created, configured and
started, and the app will show you a URL to call — but nothing in this application serves that
URL, so the bot can never fire.

This note records what is built, what is missing, and what finishing it would involve.

## What a signal bot is meant to do

Unlike a DCA bot, a signal bot has no schedule. It waits for an HTTP call from an outside system —
a TradingView alert, a script, another service — and buys or sells when that call arrives. Each
`BotSignal` row is one rule ("buy 100 USDT of BTC when triggered") with its own secret token, and
the token is the whole address: whoever knows it can fire that rule.

## What is built

Everything except the receiver:

- `Bots::Signal` (`app/models/bots/signal.rb`) — the bot type, with asset/exchange validation and
  its own `start` / `stop` / `delete`.
- `BotSignal` (`app/models/bot_signal.rb`) — the individual rules, with direction, amount, amount
  type, and a unique `token` generated on create.
- The five-step creation wizard under `app/controllers/bots/signals/`, linked from the bot-type
  picker on `/bots/new`.
- `Bots::BotSignalsController` for adding, editing and removing rules on an existing bot.
- The signal widget (`app/views/bots/signals/_signal_widget.html.erb`), which renders
  `request.base_url + signal.webhook_url` as the URL to call.

## What is missing

`BotSignal#webhook_url` builds `/hook/<token>`, and that path is not routed:

```ruby
Rails.application.routes.recognize_path('/hook/abc123', method: :post)
# => ActionController::RoutingError
```

There is no route in `config/routes.rb` (the catch-all `get '*path'` is commented out), no
controller, and no `find_by(token:)` anywhere in `app/` or `lib/`. `git log -S"/hook/"` over
`config/routes.rb` and `app/controllers` returns nothing, so a receiver was never present and then
removed — it has not been written.

`Bots::Signal` carries no scheduler (`# No Bot::Lifecycle: Signal is passive (no scheduling)`), so
the webhook is the only thing that could ever trigger a trade. There is no fallback path.

## What a user sees today

A signal bot can be created and started, reports itself as `scheduled`, and displays a webhook URL
that returns 404. It never trades, and nothing reports that it cannot.

## Finishing it

The plumbing is small: a route outside the locale scope (`/csp-report` is the existing example of
one), a controller that looks the token up, and a job that places the order through the same path
the other bot types use.

The design questions are the ones any trade-triggering webhook has, and they are why this should
not be added casually:

- **Authentication.** The token is a bearer secret in a URL. URLs end up in logs, browser history
  and third-party alert configurations. Decide whether the token alone may authorise spending
  money, or whether a signed body or a second factor is required.
- **Replay.** An endpoint that places an order must not place two when a sender retries. The
  `Idempotency` concern (`app/controllers/concerns/idempotency.rb`) is the existing mechanism.
- **Rate limiting and abuse.** The endpoint is unauthenticated in the ordinary sense and publicly
  reachable; a leaked token is a way to drain an account by repetition.
- **Reporting.** A rejected or ignored call should be visible to the account owner — a signal that
  silently does nothing is the failure mode this feature already has.

Until those are settled, the alternative is to remove the bot type rather than continue offering a
URL that does not answer.

## Why signal bots are absent from the API

`create_signal_bot` is deliberately not among the MCP/REST tools. A tool that creates signal bots
would hand out webhook URLs that nothing serves, which is worse than not offering it at all. See
the "Deliberately out" section of the MCP/REST coverage work for the other scope decisions.
