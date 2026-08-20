# One bot's rebalance evaluation. Unlike Bot::ActionJob this never schedules itself — the rebalance
# leg is condition-driven, so Bot::EvaluateRebalancersJob polls and this decides.
#
# NO retry_on. A job-level retry around order placement replays the placement, which is the known
# double-buy bug class: placement has no idempotency key, so a retried "failure" that actually landed
# places a second order. Transient handling belongs inside the rebalancer, where an unknown outcome
# halts as :ambiguous instead of being replayed.
class Bot::RebalanceJob < BotJob
  # Explicitly joins Bot::ActionJob's semaphore. Solid Queue's concurrency_group defaults to
  # `self.class.name` and the key is [group, param].join("/"), so same-key different-class jobs do
  # NOT share a lock — without this a DCA tick and a rebalance tick would run against each other's
  # stale balances. Deliberately not set on BotJob itself: that would also serialize the three
  # order-fetch jobs behind trading ticks, fleet-wide.
  limits_concurrency to: 1,
                     key: ->(bot, *) { "exchange_#{bot.exchange&.name_id}" },
                     group: 'Bot::ActionJob'

  def perform(bot)
    return unless resumable?(bot)

    bot.ensure_exchange_authenticated
    return unless market_open?(bot)

    bot.rebalance!
  end

  private

  # Guards split in two on purpose. These apply to BOTH new work and resumes — they are about
  # whether we can talk to the venue at all.
  #
  # Notably absent: rebalance_enabled? and "is it still due". Once a sell is placed the buy is owed,
  # so a user toggling the widget off mid-flight, or an intermediate drift reading that no longer
  # breaches the band, must not strand the proceeds as uninvested cash. Bot::Rebalanceable#rebalance!
  # applies the new-work guards itself.
  def resumable?(bot)
    return false if bot.deleted? || bot.archived?
    return false unless bot.rebalance_enabled? || bot.rebalance_pending?
    return false if bot.api_key&.pending_activation?

    bot.tickers_for_start.all? { |ticker| ticker.present? && ticker.available? && ticker.trading_enabled? }
  end

  def market_open?(bot)
    tickers = bot.tickers.to_a
    return true if bot.exchange.market_open?(tickers: tickers)

    Rails.logger.info("RebalanceJob for bot #{bot.id}: market closed, skipping")
    false
  end
end
