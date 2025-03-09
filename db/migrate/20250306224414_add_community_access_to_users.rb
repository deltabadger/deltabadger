class AddCommunityAccessToUsers < ActiveRecord::Migration[6.0]
  def change
    add_column :users, :community_access, :boolean, default: false, null: false
  end
end
