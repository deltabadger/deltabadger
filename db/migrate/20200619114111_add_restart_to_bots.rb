class AddRestartToBots < ActiveRecord::Migration[5.2]
  def change
    add_column :bots, :restarts, :integer, null: false, default: 0
  end
end
