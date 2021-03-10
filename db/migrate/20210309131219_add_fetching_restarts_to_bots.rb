class AddFetchingRestartsToBots < ActiveRecord::Migration[5.2]
  def change
    add_column :bots, :fetch_restarts, :integer, null: false, default: 0
  end
end
