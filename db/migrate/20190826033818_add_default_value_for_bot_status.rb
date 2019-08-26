class AddDefaultValueForBotStatus < ActiveRecord::Migration[5.2]
  def change
    change_column :bots, :status, :integer, null: false, default: 0
  end
end
