module User::Sendgridable
  extend ActiveSupport::Concern

  SENDGRID_NEW_USERS_LIST_NAME       = ENV.fetch('SENDGRID_NEW_USERS_LIST').freeze
  SENDGRID_FREE_USERS_LIST_NAME      = ENV.fetch('SENDGRID_FREE_USERS_LIST').freeze
  SENDGRID_BASIC_USERS_LIST_NAME     = ENV.fetch('SENDGRID_BASIC_USERS_LIST').freeze
  SENDGRID_PRO_USERS_LIST_NAME       = ENV.fetch('SENDGRID_PRO_USERS_LIST').freeze
  SENDGRID_LEGENDARY_USERS_LIST_NAME = ENV.fetch('SENDGRID_LEGENDARY_USERS_LIST').freeze
  KRAKEN_STARTED                     = ENV.fetch('SENDGRID_KRAKEN_STARTED').freeze

  included do
    # validate :validate_email_with_sendgrid  # Disabled for now
    before_save :initialize_sendgrid_lists, if: :email_confirmed_after_user_created?
    after_save_commit -> { Sendgrid::UpdateFirstNameJob.perform_later(self) }, if: :saved_change_to_name?
  end

  def add_to_sendgrid_list(list_name)
    result = get_list_id(list_name)
    return result unless result.success?

    list_id = result.data || begin
      result = create_list(list_name)
      return result unless result.success?

      result.data
    end

    contact = {
      email: email,
      first_name: name&.split&.first&.capitalize
    }.compact
    sendgrid_client.add_or_update_contacts(list_ids: [list_id], contacts: [contact])
  end

  def add_to_sendgrid_exchange_list(exchange_name)
    list_name = self.class.const_get("#{exchange_name.upcase}_STARTED")
    add_to_sendgrid_list(list_name)
  end

  def update_sendgrid_plan_list(from_plan_name, to_plan_name)
    from_const_name = "SENDGRID_#{from_plan_name.upcase}_USERS_LIST_NAME"
    from_list_name = self.class.const_get(from_const_name)
    if from_list_name.present?
      result = remove_from_sendgrid_list(from_list_name)
      return result unless result.success?
    end

    to_const_name = "SENDGRID_#{to_plan_name.upcase}_USERS_LIST_NAME"
    to_list_name = self.class.const_get(to_const_name)
    add_to_sendgrid_list(to_list_name)
  end

  def update_sendgrid_first_name
    contact = {
      email: email,
      first_name: name&.split&.first&.capitalize || ''
    }.compact
    sendgrid_client.add_or_update_contacts(list_ids: nil, contacts: [contact])
  end

  private

  def sendgrid_client
    @sendgrid_client ||= SendgridClient.new
  end

  def get_list_id(list_name)
    result = sendgrid_client.get_all_lists
    return result unless result.success?

    list_id = result.data.fetch('result').select { |list| list['name'] == list_name }.pluck('id').first
    Result::Success.new(list_id)
  end

  def create_list(list_name)
    result = sendgrid_client.create_list(name: list_name)
    return result unless result.success?

    Result::Success.new(result.data.fetch('id'))
  end

  def remove_from_sendgrid_list(list_name)
    result = get_list_id(list_name)
    return result unless result.success?

    list_id = result.data
    result = sendgrid_client.get_contacts_by_emails(emails: [email])
    return result unless result.success?

    contact_id = Utilities::Hash.dig_or_raise(result.data, 'result', email, 'contact', 'id')
    result = sendgrid_client.remove_contacts_from_list(id: list_id, contact_ids: [contact_id])
    return result unless result.success?

    Result::Success.new
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

  def email_confirmed_after_user_created?
    confirmed_at_was.nil? && confirmed_at.present?
  end

  def initialize_sendgrid_lists
    Sendgrid::AddToListJob.perform_later(self, SENDGRID_NEW_USERS_LIST_NAME)
    Sendgrid::AddToListJob.perform_later(self, SENDGRID_FREE_USERS_LIST_NAME)
  end
end
