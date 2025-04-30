class AddMetricsAndLabelsToBots < ActiveRecord::Migration[6.0]
  def change
    add_column :bots, :metrics_status, :integer, default: 0
    add_column :bots, :label, :string
  end
end
