# The rebalance leg's clock — the only one it has. Mirrors Rule::EvaluateAllJob: fan out, let each
# bot decide for itself. The 4-hourly cadence is also the rate floor; a successful rebalance moves
# all the way to target, so the band cannot immediately re-trigger and no separate cooldown is
# needed.
# ponytail: no per-bot cooldown knob. If churn is ever observed, the shape to copy is
# Rules::Withdrawal#max_interval_elapsed?.
class Bot::EvaluateRebalancersJob < ApplicationJob
  queue_as :default
  limits_concurrency to: 1, key: -> { 'evaluate_rebalancers' }, on_conflict: :discard

  def perform
    candidates.find_each { |bot| Bot::RebalanceJob.perform_later(bot) }
  end

  private

  # Deliberately NOT the `working` scope. A stopped bot still rebalances — that is the headline of
  # the feature: the trigger is independent of the DCA schedule. Only deleted and archived bots are
  # out, and a bot with pending state is in regardless of its switch, because an owed buy must
  # complete even after the user turns rebalancing off.
  # Every multi-asset type — a single-asset bot has no allocation to drift.
  REBALANCEABLE_TYPES = %w[Bots::DcaDualAsset Bots::DcaIndex].freeze

  def candidates
    Bot.where(type: REBALANCEABLE_TYPES)
       .where.not(status: %i[deleted archived])
       .where("json_extract(settings, '$.rebalance_enabled') IN (1, 'true') " \
              "OR json_extract(transient_data, '$.rebalance_pending') IS NOT NULL")
  end
end
