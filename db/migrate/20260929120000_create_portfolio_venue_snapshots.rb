# The chart's history for one exchange, written by the same sweep as the whole portfolio's — so the
# exchange switch reads rows instead of sweeping the account again on every visit.
class CreatePortfolioVenueSnapshots < ActiveRecord::Migration[8.1]
  def change
    create_table :portfolio_venue_snapshots do |t|
      t.references :user, null: false, index: false
      t.references :exchange, null: false, index: false
      t.date :date, null: false
      t.decimal :value_usd, precision: 20, scale: 8, null: false, default: 0
      t.decimal :invested_usd, precision: 20, scale: 8, null: false, default: 0
      t.decimal :held_value_usd, precision: 20, scale: 8
      t.decimal :held_cost_usd, precision: 20, scale: 8
      t.boolean :partial, null: false, default: false

      t.timestamps
    end

    add_index :portfolio_venue_snapshots, %i[user_id exchange_id date], unique: true
  end
end
