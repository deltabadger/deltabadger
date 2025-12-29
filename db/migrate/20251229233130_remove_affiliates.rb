class RemoveAffiliates < ActiveRecord::Migration[6.0]
  def change
    remove_column :users, :referrer_id
    drop_table :affiliates
  end
end
