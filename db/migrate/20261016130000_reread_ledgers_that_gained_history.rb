# The 2026-10-16 venue fixes widened what a ledger sync reads: Bybit and Bitget page their fills back
# instead of one window, Coinbase reads every account, Gemini, MEXC, BingX, Bitvavo and Hyperliquid
# read the whole trade history and import deposits and withdrawals they never read before.
#
# A key that had synced keeps its watermark, and every later sync asks from it less the overlap, so the
# history the old code missed would stay missing. The watermark goes back to nil on those venues' keys:
# the next scheduled sync reads the full history as a fresh key does. Rows already held come back as
# duplicates and are skipped. (Bitrue's own migration did the same for Bitrue.)
class RereadLedgersThatGainedHistory < ActiveRecord::Migration[8.1]
  VENUES = %w[Bybit Bitget Coinbase Gemini Mexc Bingx Bitvavo Hyperliquid].map { |name| "Exchanges::#{name}" }.freeze

  def up
    types = VENUES.map { |type| connection.quote(type) }.join(', ')
    execute(<<~SQL.squish)
      UPDATE api_keys SET last_synced_at = NULL
      WHERE exchange_id IN (SELECT id FROM exchanges WHERE type IN (#{types}))
    SQL
  end

  def down
    # Nothing to restore: the watermark only moves back, and the next sync moves it forward again.
  end
end
