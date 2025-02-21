class AddMetricsToBots < ActiveRecord::Migration[6.0]
  def change
    add_column :bots, :metrics_status, :integer, default: 0
  end
end
