class AddStopMessageKeyToBots < ActiveRecord::Migration[6.0]
  def change
    add_column :bots, :stop_message_key, :string
  end
end
