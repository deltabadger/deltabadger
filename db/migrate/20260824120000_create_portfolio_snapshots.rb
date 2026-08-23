class CreatePortfolioSnapshots < ActiveRecord::Migration[8.1]
  def change
    create_table :portfolio_snapshots do |t|
      t.references :user, null: false
      t.date :date, null: false
      t.decimal :value_usd, precision: 20, scale: 8, null: false, default: 0
      t.decimal :invested_usd, precision: 20, scale: 8, null: false, default: 0
      # A day the figures could not be stated in full: a held asset with no price, a key that
      # failed to sync, or prices older than the balances beside them.
      t.boolean :partial, null: false, default: false

      t.timestamps
    end

    add_index :portfolio_snapshots, %i[user_id date], unique: true
  end
end
