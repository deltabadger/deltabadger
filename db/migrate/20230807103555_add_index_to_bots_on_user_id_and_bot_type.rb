class AddIndexToBotsOnUserIdAndBotType < ActiveRecord::Migration[6.0]
  def change
    add_index :users, :referrer_id
    add_index :bots, [:user_id, :bot_type]
    change_column :bots, :bot_type, :integer, using: 'bot_type::integer'
  end
end
