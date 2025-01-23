class AddSendgridUnsubscribedToUsers < ActiveRecord::Migration[6.0]
  def change
    add_column :users, :sendgrid_unsubscribed, :boolean, default: false
  end
end
