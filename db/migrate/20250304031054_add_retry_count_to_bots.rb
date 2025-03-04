class AddRetryCountToBots < ActiveRecord::Migration[6.0]
  def change
    add_column :bots, :retry_count, :integer, default: 0
  end
end
