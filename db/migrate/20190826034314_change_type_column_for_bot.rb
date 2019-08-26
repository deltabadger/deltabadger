class ChangeTypeColumnForBot < ActiveRecord::Migration[5.2]
  def change
    remove_column :bots, :type
    add_column :bots, :bot_type, :integer
  end
end
