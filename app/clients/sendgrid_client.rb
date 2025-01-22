class SendgridClient < ApplicationClient
  URL = 'https://api.sendgrid.com'.freeze
  API_KEY = ENV.fetch('SENDGRID_VALIDATION_API_KEY').freeze # TODO: Change to SENDGRID_API_KEY

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.headers = {
        'Content-Type': 'application/json',
        'Authorization': "Bearer #{API_KEY}"
      }
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: true, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://www.twilio.com/docs/sendgrid/api-reference/contacts/add-or-update-a-contact
  # @param list_ids [Array<String>] An array of List ID strings that this contact will be added to.
  # @param contacts [Array<Hash>] One or more contacts objects that you intend to upsert.
  #                               Each contact needs to include at least one of email, phone_number_id,
  #                               external_id, or anonymous_id as an identifier.
  def add_or_update_contacts(list_ids:, contacts:)
    with_rescue do
      response = self.class.connection.put do |req|
        req.url '/v3/marketing/contacts'
        req.body = {
          list_ids: list_ids,
          contacts: contacts
        }
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.twilio.com/docs/sendgrid/api-reference/contacts/delete-contacts
  # @param ids [Array<String>] A list of contact IDs.
  # @param delete_all_contacts [Boolean] Must be set to "true" to delete all contacts.
  def delete_contacts(ids:)
    with_rescue do
      response = self.class.connection.delete do |req|
        req.url '/v3/marketing/contacts'
        req.params = {
          ids: ids.join(',')
          # delete_all_contacts: false # Warning! Disabled for now as it can delete all contacts.
        }
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.twilio.com/docs/sendgrid/api-reference/contacts/get-contacts-by-emails
  # @param emails [Array<String>] One or more primary and/or alternate email addresses to search for in your
  #                               Marketing Campaigns contacts.
  # @param phone_number_id [String] The contact's Phone Number ID. This is required to be a valid phone number.
  # @param external_id [String] The contact's External ID.
  # @param anonymous_id [String] The contact's Anonymous ID.
  def get_contacts_by_emails(emails:, phone_number_id: nil, external_id: nil, anonymous_id: nil)
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/v3/marketing/contacts/search/emails'
        req.body = {
          emails: emails,
          phone_number_id: phone_number_id,
          external_id: external_id,
          anonymous_id: anonymous_id
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.twilio.com/docs/sendgrid/api-reference/lists/get-a-list-by-id
  # @param id [String] The ID of the list on which you want to perform the operation.
  # @param contact_sample [Boolean] Setting this parameter to the true will cause the contact_sample to be returned.
  def get_list_by_id(id:, contact_sample: false)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "/v3/marketing/lists/#{id}"
        req.params = {
          contact_sample: contact_sample
        }
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.twilio.com/docs/sendgrid/api-reference/lists/get-all-lists
  # @param page_size [Integer] Maximum number of elements to return. Defaults to 100, returns 1000 max.
  # @param page_token [String]
  def get_all_lists(page_size: 100, page_token: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v3/marketing/lists'
        req.params = {
          page_size: page_size,
          page_token: page_token
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.twilio.com/docs/sendgrid/api-reference/lists/create-list
  # param name [String] The name of the list to create.
  def create_list(name:)
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/v3/marketing/lists'
        req.body = {
          name: name
        }
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.twilio.com/docs/sendgrid/api-reference/lists/remove-contacts-from-a-list
  # @param id [String] The ID of the list on which you want to perform the operation.
  # @param contact_ids [Array<String>] An array of contact IDs to add to the list.
  def remove_contacts_from_list(id:, contact_ids:)
    with_rescue do
      response = self.class.connection.delete do |req|
        req.url "/v3/marketing/lists/#{id}/contacts"
        req.params = {
          contact_ids: contact_ids.join(',')
        }
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.twilio.com/docs/sendgrid/api-reference/email-address-validation/validate-an-email
  # @param email [String] The email address that you want to validate.
  # @param source [String] A one-word classifier for where this validation originated.
  def validate_email(email:, source: nil)
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/v3/validations/email'
        req.body = {
          email: email,
          source: source
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.twilio.com/docs/sendgrid/api-reference/suppressions-suppressions/retrieve-all-suppressions
  # @param id [String] The ID of the list on which you want to perform the operation.
  # @param contact_sample [Boolean] Setting this parameter to the true will cause the contact_sample to be returned.
  def retrieve_all_suppressions
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v3/asm/suppressions'
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.twilio.com/docs/sendgrid/api-reference/suppressions-global-suppressions/retrieve-all-global-suppressions
  # @param id [String] The ID of the list on which you want to perform the operation.
  # @param contact_sample [Boolean] Setting this parameter to the true will cause the contact_sample to be returned.
  def retrieve_all_global_suppressions(start_time: nil, end_time: nil, limit: 500, offset: 0, email: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v3/suppression/unsubscribes'
        req.params = {
          start_time: start_time,
          end_time: end_time,
          limit: limit,
          offset: offset,
          email: email
        }.compact
      end
      Result::Success.new(response.body)
    end
  end
end
