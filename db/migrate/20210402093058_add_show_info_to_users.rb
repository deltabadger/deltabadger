class AddShowInfoToUsers < ActiveRecord::Migration[5.2]
  def change
    add_column :users, :show_smart_intervals_info, :boolean, null: false, default: true
  end
end
