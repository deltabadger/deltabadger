# Signal bots

A signal bot has no schedule. It waits for an HTTP call from an outside system — a TradingView
alert, a script, another service — and buys or sells when that call arrives. Each rule
(`BotSignal`) on the bot is one instruction ("buy 100 USDT of BTC when triggered") with its own
secret token, and the token is the whole address: whoever knows it can fire that rule.

## Calling a rule

Each rule's URL is shown on its widget: `https://<host>/hook/<token>`. `POST` it. The request body
is never read — the rule defines the trade, the URL is only the trigger — so any sender that can
make an HTTP call works, whatever it puts in the body.

| response | meaning |
|---|---|
| `202 {"status":"accepted"}` | the order is being placed |
| `200 {"status":"ignored","reason":"cooldown"}` | the rule already fired in the last 30 seconds |
| `200 {"status":"ignored","reason":"bot_not_running"}` | the bot is stopped or archived |
| `200 {"status":"ignored","reason":"signal_disabled"}` | the rule is switched off |
| `404` | unknown token |
| `503` | the queue refused the call; try again |

Ignored calls are acknowledged rather than refused: a sender that treats non-2xx as a failed
delivery (TradingView does) must not report a call we chose to drop as an error on its side. The
widget shows when the webhook was last triggered, which is the trace an ignored call leaves.

## What a call does

- **Sizing.** A buy for a fixed amount spends that much of the quote asset; a percentage buy spends
  that share of the spendable quote balance. A sell for a fixed amount is quote-denominated, sized
  in base, and never sells more than is free; a percentage sell sells that share of the free base
  balance. Market orders only, sized at the price the venue says a market order executes at.
- **Every accepted call leaves one visible row**: a submitted, skipped or failed transaction in the
  feed, or an activity line saying why nothing was placed — market closed, API key pending
  activation, the call waited too long in the queue, or the bot was stopped or the rule switched
  off before it ran.
- **Email** on the first failure after a success, not on every one.

## How it is built

- `HooksController` (`POST /hook/:token`, outside the locale scope, `ActionController::API`): one
  lookup, one atomic 30-second claim per rule (`BotSignal#claim_trigger!`, a conditional UPDATE on
  `last_triggered_at`), one enqueue. A queue refusal hands the claim back and answers 503. The
  token is masked in the request log line.
- `Bot::SignalJob`: one-shot, no `retry_on` (a retried placement is the double-buy bug class —
  `Bot::RebalanceJob` sets the precedent), shares `Bot::ActionJob`'s per-exchange semaphore, drops
  a call older than five minutes.
- `Bots::Signal::OrderSetter#execute_signal`: sizing, placement and recording. Failures are
  classified by where they happen, because that decides whether money moved: before the placement
  call → a failed row; inside it → `placement_ambiguous` (the request may have gone out; never
  retried, never written down as failed); after acceptance → the submitted row stays whatever else
  goes wrong. `Exchange#ambiguous_placement_error?` is the classifier for a failed placement
  result, and `Exchange#acknowledged_order_id?` for a success that carries no real id.

## Deliberately out

- A second factor or signed body: alert systems can produce neither. A per-sender secret in the
  body would be a second bearer in the same request.
- An IP throttle: behind a CDN the throttle key names the edge, which would let a stranger
  rate-limit the user's own alerts. The per-rule claim is the bound.
- Body-carried parameters, limit orders, and `create_signal_bot` on MCP/REST (see the
  "Deliberately out" section of the MCP/REST coverage work).
