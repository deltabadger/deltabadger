class ResetAlpacaSyncWatermarks < ActiveRecord::Migration[8.1]
  # The old clock-based watermark may have skipped unfetched history; id-stable dedup makes a full re-pull safe.
  def up
    execute("UPDATE api_keys SET last_synced_at = NULL WHERE exchange_id IN (SELECT id FROM exchanges WHERE type = 'Exchanges::Alpaca')")
  end

  def down; end
end
