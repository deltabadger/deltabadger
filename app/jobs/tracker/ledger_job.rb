module Tracker
  # Warms the tracker ledger for one scope and refreshes whoever is looking at it. Building it
  # prices every unpriced row, so it belongs here and never in a request.
  class LedgerJob < ApplicationJob
    queue_as :low_priority
    limits_concurrency to: 1, key: ->(user_id, exchange_id = nil) { "tracker_ledger_#{user_id}_#{exchange_id}" },
                       on_conflict: :discard

    def perform(user_id, exchange_id = nil)
      user = User.find(user_id)
      summary = Tracker::Ledger.compute!(user, exchange: exchange_id && Exchange.find(exchange_id))
      # Today's snapshot is half balances and half ledger, written by whichever sync finishes last.
      # The balance job can easily beat the transaction one, so the row it left carries yesterday's
      # invested figure until this rewrites it.
      PortfolioSnapshot.record!(user) if exchange_id.nil?
      arm_wash_sale_locks(user, summary) if exchange_id.nil?
      Turbo::StreamsChannel.broadcast_refresh_to("user_#{user_id}", :sync)
    end

    private

    # A sale is a sale whoever made it — including one made on the exchange's own website, which no
    # bot ever sees. Re-derived on every run rather than fired when a row arrives: idempotent,
    # self-healing, and it covers sales made before the feature existed or while the rule was off.
    #
    # The summary is the one compute! already returned. Calling Tracker::Ledger.for here instead
    # would walk the whole account history a second time, on every sync.
    #
    # Symbols resolve through Ticker#base_asset_id and NOT Tracker::Ledger.asset_index: that one is
    # built from AccountBalance rows, and a fully liquidated position — exactly the harvest this
    # feature exists to protect — has none, so it would arm nothing. A symbol matching two tickers
    # locks both asset ids; over-locking is the safe direction.
    def arm_wash_sale_locks(user, summary)
      return if user.wash_sale_days.zero? || summary.loss_sales.blank?

      by_symbol = Ticker.where(base: summary.loss_sales.keys)
                        .pluck(:base, :base_asset_id)
                        .group_by(&:first)
                        .transform_values { |rows| rows.map(&:last).uniq }

      summary.loss_sales.each do |symbol, on|
        Array(by_symbol[symbol]).each do |asset_id|
          WashSaleLock.confirm!(user: user, asset_id: asset_id, from: on, source: 'ledger')
        end
      end
    end
  end
end
