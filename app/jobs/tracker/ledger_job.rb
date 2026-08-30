module Tracker
  # Warms the tracker ledger for one scope and refreshes whoever is looking at it. Building it
  # prices every unpriced row, so it belongs here and never in a request.
  class LedgerJob < ApplicationJob
    queue_as :low_priority
    limits_concurrency to: 1, key: ->(user_id, exchange_id = nil) { "tracker_ledger_#{user_id}_#{exchange_id}" },
                       on_conflict: :discard

    def perform(user_id, exchange_id = nil)
      user = User.find(user_id)
      Tracker::Ledger.compute!(user, exchange: exchange_id && Exchange.find(exchange_id))
      # Today's snapshot is half balances and half ledger, written by whichever sync finishes last.
      # The balance job can easily beat the transaction one, so the row it left carries yesterday's
      # invested figure until this rewrites it.
      PortfolioSnapshot.record!(user) if exchange_id.nil?
      Turbo::StreamsChannel.broadcast_refresh_to("user_#{user_id}", :sync)
    end
  end
end
