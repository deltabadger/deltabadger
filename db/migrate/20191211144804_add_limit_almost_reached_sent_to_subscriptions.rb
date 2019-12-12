class AddLimitAlmostReachedSentToSubscriptions < ActiveRecord::Migration[5.2]
  def change
    add_column :subscriptions, :limit_almost_reached_sent, :boolean, default: false
  end
end
