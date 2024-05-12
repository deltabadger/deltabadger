class CreatePortfolios < ActiveRecord::Migration[6.0]
  def change
    create_table :portfolios do |t|
      t.references :user, null: false, foreign_key: true
      t.integer :strategy, default: 0.0, null: false
      t.boolean :smart_allocation_on, default: false, null: false
      t.integer :risk_level, default: 2, null: false
      t.integer :benchmark, default: 0.0, null: false
      t.string :backtest_start_date

      t.timestamps
    end
  end
end
