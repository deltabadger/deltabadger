class SendgridMailToList < BaseService
  LISTS_URL = 'https://api.sendgrid.com/v3/marketing/lists'.freeze
  CONTACTS_URL = 'https://api.sendgrid.com/v3/marketing/contacts'.freeze
  API_KEY = ENV.fetch('SENDGRID_VALIDATION_API_KEY').freeze
  LIST_NAME = ENV.fetch('SENDGRID_NEW_USERS_LIST').freeze


  def call(user)
    if list_ids.present?
      add_email_to_list(user)
    else
      new_list_id = create_list_id
      add_email_to_list(user, [new_list_id])
    end
  rescue StandardError => e
    Raven.capture_exception(e)
    nil
  end

  private

  def list_ids
    @list_ids ||= begin
      response = JSON.parse(Faraday.get(LISTS_URL, {}, headers).body)

      result = response.fetch('result', nil)

      result.select{ |list| list["name"] == LIST_NAME }.pluck('id') if result.present?
    end
  end

  def create_list_id
    response = Faraday.post(LISTS_URL, new_list_request_body.to_json, headers)
    body = JSON.parse(response.body)

    raise StandardError, body["errors"] unless response.status == 201

    body.fetch('id')
  end

  def add_email_to_list(user, email_list_ids = list_ids)
    response = Faraday.put(CONTACTS_URL, add_email_request_body(user.email, user.name, email_list_ids).to_json, headers)
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

  def new_list_request_body
    {
        'name' => LIST_NAME
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
