class RenameSubscriptionEndTime < ActiveRecord::Migration[6.0]
  def change
    rename_column :subscriptions, :end_time, :ends_at
  end
end
