class DropUnusedTables < ActiveRecord::Migration[8.1]
  def up
    # Remove foreign keys from users table that reference tables being dropped
    remove_foreign_key :users, column: :referrer_id
    remove_foreign_key :users, column: :pending_plan_variant_id

    # Remove the columns from users table
    remove_column :users, :referrer_id
    remove_column :users, :pending_plan_variant_id

    # Drop tables (order matters due to foreign key constraints)
    drop_table :caffeinate_mailings
    drop_table :caffeinate_campaign_subscriptions
    drop_table :caffeinate_campaigns
    drop_table :cards
    drop_table :payments
    drop_table :portfolio_assets
    drop_table :portfolios
    drop_table :affiliates
    drop_table :ahoy_messages
    drop_table :ahoy_clicks
    drop_table :ahoy_opens
    drop_table :articles
    drop_table :authors
    drop_table :subscriptions
    drop_table :subscription_plan_variants
    drop_table :subscription_plans
    drop_table :conversion_rates
    drop_table :countries
    drop_table :surveys
  end

  def down
    raise ActiveRecord::IrreversibleMigration
  end
end
