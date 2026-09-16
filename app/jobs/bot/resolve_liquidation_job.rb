# Clears a halted liquidation once the user attests they checked the venue.
#
# Runs as a job rather than inline in the controller specifically so it takes Bot::ActionJob's
# semaphore: clearing straight from a request could race a placement that is still in flight — the
# user clears, the placement then comes back ambiguous, and the halt it should have raised has
# already been wiped.
#
# Two things can need accounting for, and one attestation covers both: an intent whose placement
# never reported an id, and orders the venue stopped reporting. Each is resolved on its own terms —
# the intent on its generation nonce, the orders on the exact ids the page was showing — so anything
# that went unaccounted-for AFTER the render is untouched and goes on blocking.
class Bot::ResolveLiquidationJob < BotJob
  limits_concurrency to: 1,
                     key: ->(bot, *) { "exchange_#{bot.exchange&.name_id}" },
                     group: 'Bot::ActionJob'

  def perform(bot, intent_id:, order_ids: [], user_id: nil)
    # Read before it is cleared, since afterwards there is nothing left to name.
    intent_base = bot.liquidation_pending&.dig(:symbol)
    resolved = bot.resolve_liquidation_orders!(Array(order_ids).map(&:to_i))
    cleared = clear_intent?(bot, intent_id)
    bot.clear_liquidation_pending! if cleared
    return if !cleared && resolved.empty?

    # What was ACTUALLY accounted for, not what was halted. A stale page can submit one order while
    # another has since been given up on; that one is correctly still blocking, and naming it here
    # would put an attestation in the feed that the user never gave.
    accounted = ((cleared ? [intent_base] : []) + resolved.map(&:base)).compact_blank.uniq
    bot.log_activity('liquidation_manually_resolved', level: :info,
                                                      details: { user_id: user_id, base: accounted.join(', ').presence })
    # Nothing else repaints the widget — trading was blocked while halted — so without this the page
    # keeps showing Clear after the halt is gone.
    bot.broadcast_liquidation_state
  end

  private

  def clear_intent?(bot, intent_id)
    pending = bot.liquidation_pending
    return false if pending.nil?
    # Generation check: a resolution queued against an EARLIER halt — a stale tab, a double click —
    # must not clear a later one, wiping an attestation the user never gave for that event.
    return false unless pending[:id] == intent_id

    # A placement may still be running for a `placing` intent, and an attestation about it would be
    # about an outcome that has not happened yet.
    bot.liquidation_ambiguous?
  end
end
