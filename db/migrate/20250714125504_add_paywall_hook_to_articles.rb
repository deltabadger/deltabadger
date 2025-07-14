class AddPaywallHookToArticles < ActiveRecord::Migration[6.0]
  def change
    add_column :articles, :paywall_hook, :text
  end
end
