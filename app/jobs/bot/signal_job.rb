# One webhook call, one market order. Enqueued by HooksController once a rule's claim is won; the
# sizing and placement live in Bots::Signal::OrderSetter, this decides whether they may run at all.
#
# NO retry_on. A job-level retry around order placement replays the placement, which is the known
# double-buy bug class (Bot::RebalanceJob, Bot::LiquidateExitedJob). Reads retry in place inside
# the order setter; an unknown outcome is logged as ambiguous, never replayed. Nothing is re-raised
# either: a one-shot placement job has nothing an operator's Retry could safely do, so the rows,
# the activity lines, the email and the error log are the record.
class Bot::SignalJob < BotJob
  # A call that waited longer than this behind the per-exchange semaphore is not the trade that was
  # asked for. Dropped visibly — the sender was told the call was accepted.
  # ponytail: one fixed window; make it a rule setting if a trader wants a stricter or looser one.
  MAX_AGE = 5.minutes

  # Explicitly joins Bot::ActionJob's semaphore: Solid Queue keys the lock on [group, key], so
  # without the group a signal would run against a DCA tick on the same exchange account, both
  # sizing off the same free balance.
  limits_concurrency to: 1,
                     key: ->(bot, *) { "exchange_#{bot.exchange&.name_id}" },
                     group: 'Bot::ActionJob'

  # The rule was removed between the call and the run. There is nothing to retry.
  discard_on ActiveJob::DeserializationError

  def perform(bot, signal, triggered_at)
    return refuse(bot, signal, 'signal_expired') if triggered_at < MAX_AGE.ago
    # The claim was won at the door; the world may have moved since.
    return refuse(bot, signal, 'signal_ignored') unless bot.working? && signal.enabled?
    # An IBKR key registered but not yet activated must never reach a live IBKR call.
    return refuse(bot, signal, 'signal_api_key_pending') if bot.api_key&.pending_activation?

    bot.ensure_exchange_authenticated
    return refuse(bot, signal, 'signal_market_closed') unless bot.exchange.market_open?(tickers: bot.tickers.to_a)

    bot.execute_signal(signal)
  rescue StandardError => e
    # Only the checks above can raise into here — the order setter answers for its own phases —
    # and nothing has reached the venue yet.
    Rails.logger.error("SignalJob for bot #{bot.id}: #{e.class}: #{e.message}")
    bot.record_signal_failure(signal, [e.message])
  end

  private

  def refuse(bot, signal, event)
    Rails.logger.info("SignalJob for bot #{bot.id}: #{event} (signal #{signal.id})")
    bot.log_activity(event, details: { signal_id: signal.id })
  end
end
