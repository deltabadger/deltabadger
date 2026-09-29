module Tracker
  # Warms the tracker ledger — every scope, in one walk — and refreshes whoever is looking at it. Building it
  # prices every unpriced row, so it belongs here and never in a request.
  class LedgerJob < ApplicationJob
    queue_as :low_priority
    # Per user: one walk states every venue. The second argument is what an exchange-scoped run was
    # enqueued with before that, still read so a job already in the queue runs.
    limits_concurrency to: 1, key: ->(user_id, *) { "tracker_ledger_#{user_id}" }, group: 'Tracker::LedgerJob',
                       on_conflict: :discard

    # The run a job asks for when rows kept arriving through every pass of its own walk: what it
    # arms from is not the current ledger. A plain LedgerJob enqueued now would be DISCARDED by the
    # guard this one still holds, whenever it came due; this one shares the guard and WAITS for it,
    # so the sale that arrived is still armed.
    class Retry < LedgerJob
      limits_concurrency to: 1, key: ->(user_id, *) { "tracker_ledger_#{user_id}" }, group: 'Tracker::LedgerJob',
                         on_conflict: :block
    end

    def perform(user_id, _exchange_id = nil)
      user = User.find(user_id)
      scopes = Tracker::Ledger.compute!(user) { Retry.perform_later(user_id) }
      # Today's snapshot is half balances and half ledger, written by whichever sync finishes last.
      # The balance job can easily beat the transaction one, so the row it left carries yesterday's
      # invested figure until this rewrites it. From the ledger just computed, not a second walk.
      PortfolioSnapshot.record!(user, scopes: scopes)
      PortfolioSnapshot::BackfillJob.perform_later(user_id) if PortfolioSnapshot.history_stale?(user)
      arm_wash_sale_locks(user, scopes[nil])
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
    # for drawing — a symbol whose rows recorded two assets resolves to neither, and a row that
    # recorded none falls back to AccountBalance rows, which a fully liquidated position (exactly the
    # harvest this feature exists to protect) does not have. A symbol matching two tickers locks
    # both asset ids; over-locking is the safe direction.
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
