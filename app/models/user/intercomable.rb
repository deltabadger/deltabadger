module User::Intercomable
  extend ActiveSupport::Concern

  def update_intercom_subscription
    intercom_custom_data.self[:subscription] = subscription&.name&.capitalize
    intercom_custom_data.self[:subscription_ends_at] = subscription&.ends_at
  end

  def update_intercom_email_verified
    intercom_custom_data.self[:email_verified] = confirmed_at.present?
  end
end
