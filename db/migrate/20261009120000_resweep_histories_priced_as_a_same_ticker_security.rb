# A broker can list a security and a coin under one ticker (Alpaca's BTC beside BTC/USD). Until a
# holding was valued by the asset its rows recorded, the history sweep priced every symbol at a
# stock venue as the security whenever the catalogue had one under that ticker — a coin included.
# A complete history whose rows have not changed is never swept again on its own, so this clears
# the stamp of what each affected history was swept from (`PortfolioSnapshot.history_version_key`):
# the next tracker load finds it stale and sweeps it under the current rule. Nothing else is
# touched — the stored days stay until that sweep rewrites them.
#
# Affected: every user with a row at a stock venue naming (as its base, quote or fee) a symbol the
# catalogue has both as a security and as something else. Broader than the users the old rule
# actually got wrong (one holding only the security is swept again for nothing), never narrower.
# Clearing an absent stamp is a no-op, so running it twice costs at most one more sweep.
class ResweepHistoriesPricedAsASameTickerSecurity < ActiveRecord::Migration[8.1]
  STOCK_CATEGORIES = PortfolioSnapshot::BackfillJob::STOCK_CATEGORIES

  def up
    stocks = Asset.where(category: STOCK_CATEGORIES).distinct.pluck(:symbol)
    shared = Asset.where(symbol: stocks).where.not(category: STOCK_CATEGORIES)
                  .or(Asset.where(symbol: stocks, category: nil)).distinct.pluck(:symbol)
    return if shared.empty?

    users = AccountTransaction.joins(:exchange).where(exchanges: { type: Exchange::STOCK_TYPES })
                              .where('account_transactions.base_currency IN (:s) OR account_transactions.quote_currency IN (:s) ' \
                                     'OR account_transactions.fee_currency IN (:s)', s: shared)
                              .distinct.pluck(:user_id)
    keys = users.map { |id| "#{PortfolioSnapshot::HISTORY_VERSION_KEY}_#{id}" }
    AppConfig.where(key: keys).delete_all
  end

  def down
    # Nothing to restore: the next sweep stamps each history again.
  end
end
