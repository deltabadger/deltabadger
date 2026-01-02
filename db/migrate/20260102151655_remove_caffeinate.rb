class RemoveCaffeinate < ActiveRecord::Migration[6.0]
  def change
    drop_table :caffeinate_mailings
    drop_table :caffeinate_campaign_subscriptions
    drop_table :caffeinate_campaigns
  end
end
