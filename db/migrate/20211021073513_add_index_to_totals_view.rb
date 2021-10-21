class AddIndexToTotalsView < ActiveRecord::Migration[5.2]
  def change
    add_index :bots_total_amounts, :bot_id, unique: true
  end
end
