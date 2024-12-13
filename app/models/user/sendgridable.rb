module User::Sendgridable
  extend ActiveSupport::Concern

  SENDGRID_NEW_USERS_LIST_NAME       = ENV.fetch('SENDGRID_NEW_USERS_LIST').freeze
  SENDGRID_BASIC_USERS_LIST_NAME     = ENV.fetch('SENDGRID_BASIC_USERS_LIST').freeze
  SENDGRID_PRO_USERS_LIST_NAME       = ENV.fetch('SENDGRID_PRO_USERS_LIST').freeze
  SENDGRID_LEGENDARY_USERS_LIST_NAME = ENV.fetch('SENDGRID_LEGENDARY_USERS_LIST').freeze
  KRAKEN_STARTED                     = ENV.fetch('SENDGRID_KRAKEN_STARTED').freeze

  included do
    # validate :validate_email_with_sendgrid  # Disabled for now

    def add_to_sendgrid_new_users_list
      add_to_sendgrid_list(SENDGRID_NEW_USERS_LIST_NAME)
    end

    def add_to_sendgrid_exchange_list(exchange_name)
      list_name = self.class.const_get("#{exchange_name.upcase}_STARTED")
      add_to_sendgrid_list(list_name)
    end

    def change_sendgrid_plan_list(from_plan_name, to_plan_name)
      from_const_name = "SENDGRID_#{from_plan_name&.upcase}_USERS_LIST_NAME"
      from_list_name = self.class.const_defined?(from_const_name) ? self.class.const_get(from_const_name) : nil
      remove_from_sendgrid_list(from_list_name)

      to_const_name = "SENDGRID_#{to_plan_name&.upcase}_USERS_LIST_NAME"
      to_list_name = self.class.const_defined?(to_const_name) ? self.class.const_get(to_const_name) : nil
      add_to_sendgrid_list(to_list_name)
    end
  end

  private

  def sendgrid_client
    @sendgrid_client ||= SendgridClient.new
  end

  def add_to_sendgrid_list(list_name)
    Sendgrid::AddEmailToListJob.perform_later(email, list_name, name)
  end

  def remove_from_sendgrid_list(list_name)
    Sendgrid::RemoveEmailFromListJob.perform_later(email, list_name)
  end

  def validate_email_with_sendgrid
    valid_email = if sendgrid_email_validation_result.nil?
                    false
                  else
                    sendgrid_email_validation_result.dig('result', 'verdict') == 'Valid'
                  end

    errors.add(:email, :invalid) if valid_email.nil? || !valid_email
    Rails.logger.info("Sendgrid email validation result for #{email}: #{valid_email.inspect}")
    valid_email
  end

  # def get_email_suggestion
  #   return unless devise_mapping.validatable?
  #   return if sendgrid_email_validation_result.nil?

  #   local = sendgrid_email_validation_result.dig('result', 'local')
  #   suggestion = sendgrid_email_validation_result.dig('result', 'suggestion')
  #   return if local.nil? || suggestion.nil?

  #   suggestion
  # end

  def sendgrid_email_validation_result
    cache_key = "sendgrid_email_validation_#{email}"
    Rails.cache.fetch(cache_key, expires_in: 1.day, skip_nil: true) do
      result = sendgrid_client.validate_email(email: email)
      result.failure? ? nil : result.data
    end
  end
end
