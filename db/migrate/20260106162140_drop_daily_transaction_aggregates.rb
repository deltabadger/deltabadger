class DropDailyTransactionAggregates < ActiveRecord::Migration[8.1]
  def up
    # Drop materialized view if PostgreSQL (SQLite doesn't have this)
    if connection.adapter_name == 'PostgreSQL'
      execute "DROP MATERIALIZED VIEW IF EXISTS bots_total_amounts"
    end

    drop_table :daily_transaction_aggregates, if_exists: true
  end

  def down
    raise ActiveRecord::IrreversibleMigration, "Cannot restore daily_transaction_aggregates - feature removed"
  end
end

