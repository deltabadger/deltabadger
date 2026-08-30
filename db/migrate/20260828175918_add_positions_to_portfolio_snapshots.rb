# The curve follows the "Show cash" switch, so each day carries the second reading too: what the
# positions held that day were worth, and what they cost. NULL means a day swept before the switch
# reached the chart — the sweep fills it in, it is never a figure of zero.
class AddPositionsToPortfolioSnapshots < ActiveRecord::Migration[8.1]
  def change
    add_column :portfolio_snapshots, :held_value_usd, :decimal, precision: 20, scale: 8
    add_column :portfolio_snapshots, :held_cost_usd, :decimal, precision: 20, scale: 8
  end
end
