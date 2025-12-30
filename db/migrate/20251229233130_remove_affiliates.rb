class RemoveAffiliates < ActiveRecord::Migration[6.0]
  def change
    remove_column :users, :referrer_id
    remove_column :users, :referral_banner_dismissed
    drop_table :affiliates
  end
end
