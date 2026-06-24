# One-off backfill: Hyperliquid userFills returns the spot pair index (e.g. "@333") as the
# fill coin, so historical AccountTransaction rows stored base_currency = "@333" instead of
# the token symbol ("MU"). The going-forward fix lives in Exchanges::Hyperliquid#get_ledger;
# this relabels existing rows via the ticker table. Raw SQL only (no models/callbacks).
# Idempotent: relabeled rows stop matching '@%', and unknown/delisted pairs (no ticker) are
# left untouched.
class RelabelHyperliquidTransactionBaseCurrency < ActiveRecord::Migration[8.1]
  def up
    execute(<<~SQL.squish)
      UPDATE account_transactions
      SET base_currency = (
        SELECT t.base FROM tickers t
        WHERE t.exchange_id = account_transactions.exchange_id
          AND t.ticker = account_transactions.base_currency
        LIMIT 1
      )
      WHERE exchange_id IN (SELECT id FROM exchanges WHERE type = 'Exchanges::Hyperliquid')
        AND base_currency LIKE '@%'
        AND EXISTS (
          SELECT 1 FROM tickers t
          WHERE t.exchange_id = account_transactions.exchange_id
            AND t.ticker = account_transactions.base_currency
        )
    SQL
  end

  def down
    # Irreversible data normalization. The raw "@<index>" is still present in
    # account_transactions.raw_data['coin'] if a row ever needs reconstructing, so this is a no-op.
  end
end
