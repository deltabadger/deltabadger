class SendgridMailToList < BaseService
  LISTS_URL = 'https://api.sendgrid.com/v3/marketing/lists'.freeze
  CONTACTS_URL = 'https://api.sendgrid.com/v3/marketing/contacts'.freeze
  API_KEY = ENV.fetch('SENDGRID_VALIDATION_API_KEY').freeze
  LIST_NAME = ENV.fetch('SENDGRID_NEW_USERS_LIST').freeze
  INVESTOR_LIST_NAME = ENV.fetch('SENDGRID_INVESTORS_LIST').freeze
  HODLER_LIST_NAME = ENV.fetch('SENDGRID_HODLERS_LIST').freeze
  LEGENDARY_BADGER_LIST_NAME = ENV.fetch('SENDGRID_LEGENDARY_BADGERS_LIST').freeze

  def call(user)
    add_user(user, LIST_NAME)
  rescue StandardError => e
    Raven.capture_exception(e)
    nil
  end

  def change_list(user, current_plan_name, new_plan_name)
    current_list_name = self.class.const_defined?("#{current_plan_name&.upcase || 'UNANNOUNCED'}_LIST_NAME") ? self.class.const_get("#{current_plan_name.upcase}_LIST_NAME") : false
    new_list_name = self.class.const_get "#{new_plan_name.upcase}_LIST_NAME"

    delete_user(user.email, current_list_name) if current_list_name && list_ids(current_list_name).present?

    add_user(user, new_list_name)
  end

  private

  def delete_user(email, list_name)
    user_id = get_user_from_list(email, list_name).pluck('id').join(',')
    response = Faraday.delete(CONTACTS_URL, {ids: user_id}, headers)
    body = JSON.parse(response.body)

    raise StandardError, body["errors"] unless response.status == 202

    body
  rescue StandardError => e
    Raven.capture_exception(e)
    nil
  end

  def add_user(user, list_name)
    if list_ids(list_name).present?
      add_email_to_list(user, list_ids(list_name))
    else
      new_list_id = create_list_id(list_name)
      add_email_to_list(user, [new_list_id])
    end
  end

  def get_list_id_by_name(list_name)
    response = Faraday.get("#{LISTS_URL}/#{list_ids(list_name)[0]}", {}, headers)
    body = JSON.parse(response.body)

    body.fetch('id', nil)
  end

  def get_list(list_name)
    response = Faraday.get("#{LISTS_URL}/#{get_list_id_by_name(list_name)}", {contact_sample: true}, headers)

    return [] unless response.status == 200

    body = JSON.parse(response.body)

    body['contact_sample']
  end

  def get_user_from_list(email, list_name)
    get_list(list_name).select {|contact| contact['email'] == email }
  end

  def list_ids(list_name = LIST_NAME)
    response = JSON.parse(Faraday.get(LISTS_URL, {}, headers).body)
    result = response.fetch('result', nil)

    result.select{ |list| list["name"] == list_name }.pluck('id') if result.present?
  end

  def create_list_id(name = LIST_NAME)
    response = Faraday.post(LISTS_URL, new_list_request_body(name).to_json, headers)
    body = JSON.parse(response.body)

    raise StandardError, body["errors"] unless response.status == 201

    body.fetch('id')
  end

  def add_email_to_list(user, email_list_ids = list_ids)
    user_first_name_only = user.name.split.first.capitalize
    response = Faraday.post(CONTACTS_URL, add_email_request_body(email, user_first_name_only).to_json, headers.merge({'list_ids': list_ids.join(',')}))
    body = JSON.parse(response.body)

    raise StandardError, body["errors"] unless response.status == 202

    body
  rescue StandardError => e
    Raven.capture_exception(e)
    nil
  end

  def headers
    {
        'Authorization' => "Bearer #{API_KEY}",
        'Content-Type' => 'application/json'
    }
  end

  def new_list_request_body(name)
    {
        'name' => name
    }
  end

  def add_email_request_body(email, name, email_list_ids)
    {
        'list_ids' => email_list_ids,
        'contacts' => [
            {
                'email' => email,
                'first_name' => name
            }
        ]
    }
  end

end