class AddCommunityAccessToUsers < ActiveRecord::Migration[6.0]
  def change
    add_column :users, :has_community_access, :boolean, default: false, null: false
  end
end
