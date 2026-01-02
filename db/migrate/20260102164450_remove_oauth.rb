class RemoveOauth < ActiveRecord::Migration[6.0]
  def change
    remove_column :users, :oauth_provider
    remove_column :users, :oauth_uid
  end
end
