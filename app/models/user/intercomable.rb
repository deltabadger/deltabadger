module User::Intercomable
  extend ActiveSupport::Concern

  included do
    after_save_commit -> { Intercom::UpdateUserEmailVerifiedJob.perform_later(self) }, if: :saved_change_to_confirmed_at?
  end

  def update_intercom_subscription
    # TODO: implement using intercom gem
    # intercom_custom_data.user[:subscription] = subscription&.name&.capitalize
    # intercom_custom_data.user[:subscription_ends_at] = subscription&.ends_at
  end

  def update_intercom_email_verified
    # TODO: implement using intercom gem
    # intercom_custom_data.user[:email_verified] = confirmed_at.present?
  end
end
