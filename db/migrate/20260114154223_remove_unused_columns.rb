class RemoveUnusedColumns < ActiveRecord::Migration[8.1]
  def change
    # Users table - unused columns
    remove_column :users, :pending_wire_transfer, :string
    remove_column :users, :referral_banner_dismissed, :boolean
    remove_column :users, :news_banner_dismissed, :boolean
    remove_column :users, :welcome_banner_dismissed, :boolean
    remove_column :users, :has_community_access, :boolean
    remove_column :users, :updates_agreement, :boolean
    remove_column :users, :terms_and_conditions, :boolean
    remove_column :users, :oauth_provider, :string
    remove_column :users, :oauth_uid, :string

    # Transactions table - unused column
    remove_column :transactions, :called_bot_type, :string
  end
end
