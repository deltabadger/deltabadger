class AddTransientDataToBots < ActiveRecord::Migration[6.0]
  def change
    add_column :bots, :transient_data, :json, default: {}, null: false
    add_column :bots, :started_at, :datetime
    add_column :bots, :stopped_at, :datetime
  end
end
