class AddFirstMonthEndingSentAtToSubscriptions < ActiveRecord::Migration[5.2]
  def change
    add_column :subscriptions, :first_month_ending_sent_at, :datetime
  end
end
