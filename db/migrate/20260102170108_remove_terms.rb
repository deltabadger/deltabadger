class RemoveTerms < ActiveRecord::Migration[6.0]
  def change
    remove_column :users, :terms_and_conditions
    remove_column :users, :updates_agreement
    remove_column :users, :subscribed_to_email_marketing
    remove_column :users, :has_community_access
  end
end
