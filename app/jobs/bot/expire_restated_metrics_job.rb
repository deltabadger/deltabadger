# A corporate action dated ahead of today is imported once and becomes effective later. The import
# moves the affected bots' cache generation, but the walk correctly declines to apply an action
# that has not happened — so what gets cached under the new key is the pre-split position, and no
# later sync bumps again because the row is by then a duplicate.
#
# So the bump is repeated at the moment the action takes effect.
class Bot::ExpireRestatedMetricsJob < ApplicationJob
  queue_as :low_priority

  def perform(user, exchange, symbol)
    AccountTransactionSync.expire_restated_bots(user: user, exchange: exchange, symbol: symbol)
  end
end
