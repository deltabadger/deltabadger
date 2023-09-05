class AddLastEndOfFundsNotificationToBots < ActiveRecord::Migration[6.0]
  def change
    add_column :bots, :last_end_of_funds_notification, :datetime
  end
end
