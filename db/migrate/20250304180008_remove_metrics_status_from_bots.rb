class RemoveMetricsStatusFromBots < ActiveRecord::Migration[6.0]
  def up
    remove_column :bots, :metrics_status
  end

  def down
    add_column :bots, :metrics_status, :integer, default: 0, null: false
  end
end
