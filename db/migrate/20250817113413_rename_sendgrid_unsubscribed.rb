class RenameSendgridUnsubscribed < ActiveRecord::Migration[6.0]
  def up
    add_column :users, :subscribed_to_email_marketing, :boolean, default: true

    ActiveRecord::Base.transaction do
      User.find_each do |user|
        subscribed_to_email_marketing = !user.sendgrid_unsubscribed
        user.update!(subscribed_to_email_marketing: subscribed_to_email_marketing)
      end
    end

    remove_column :users, :sendgrid_unsubscribed
  end

  def down
    add_column :users, :sendgrid_unsubscribed, :boolean, default: false

    ActiveRecord::Base.transaction do
      User.find_each do |user|
        sendgrid_unsubscribed = !user.subscribed_to_email_marketing
        user.update!(sendgrid_unsubscribed: sendgrid_unsubscribed)
      end
    end

    remove_column :users, :subscribed_to_email_marketing
  end
end
