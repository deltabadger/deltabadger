# Orders record the assets they traded, not only their symbols: a symbol can name two assets on one venue, a
# venue may spell an asset its own way, and symbols get renamed. Existing rows are resolved once from what their
# bot could have traded; a row that stays NULL is read by its symbol, as before.
class AddAssetIdsToTransactions < ActiveRecord::Migration[8.1]
  def up
    add_column :transactions, :base_asset_id, :integer
    add_column :transactions, :quote_asset_id, :integer

    report = Transaction::AssetBackfill.run!(connection)
    say "resolved rows: #{report[:resolved].inspect}"
    say "rows without a base asset: #{report[:unresolved_rows]}"
    report[:unresolved_combos].first(20).each do |bot_id, exchange_id, base, quote, candidates, rows|
      say "unresolved: bot #{bot_id}, exchange #{exchange_id}, #{base}/#{quote}, #{candidates} candidates, #{rows} rows",
          true
    end
  end

  def down
    remove_column :transactions, :quote_asset_id
    remove_column :transactions, :base_asset_id
  end
end
