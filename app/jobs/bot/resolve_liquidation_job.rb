# Clears a halted liquidation once the user attests they checked the venue.
#
# Runs as a job rather than inline in the controller specifically so it takes Bot::ActionJob's
# semaphore: clearing straight from a request could race a placement that is still in flight — the
# user clears, the placement then comes back ambiguous, and the halt it should have raised has
# already been wiped.
class Bot::ResolveLiquidationJob < BotJob
  limits_concurrency to: 1,
                     key: ->(bot, *) { "exchange_#{bot.exchange&.name_id}" },
                     group: 'Bot::ActionJob'

  def perform(bot, intent_id:, user_id: nil)
    pending = bot.liquidation_pending
    return if pending.nil?
    # Generation check: a resolution queued against an EARLIER halt — a stale tab, a double click —
    # must not clear a later one, wiping an attestation the user never gave for that event.
    return unless pending[:id] == intent_id
    # A placement may still be running for a `placing` intent, and an attestation about it would be
    # about an outcome that has not happened yet.
    return unless bot.liquidation_ambiguous?

    bot.clear_liquidation_pending!
    bot.log_activity('liquidation_manually_resolved', level: :info,
                                                      details: { user_id: user_id, base: pending[:symbol] })
    # Nothing else repaints the widget — trading was blocked while halted — so without this the page
    # keeps showing Clear after the halt is gone.
    bot.broadcast_liquidation_state
  end
end
