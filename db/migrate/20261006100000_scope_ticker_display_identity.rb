class ScopeTickerDisplayIdentity < ActiveRecord::Migration[8.1]
  OLD = 'index_exchange_tickers_on_unique_base_and_quote'.freeze

  def up
    remove_index :tickers, name: OLD if index_exists?(:tickers, name: OLD)
    # Only a current venue listing can restore a tombstone safely.
  end

  def down
    raise ActiveRecord::IrreversibleMigration, 'Stock and crypto display pairs may now coexist'
  end
end
