class ZapierMailToList < BaseService
  HOOK_URL = ENV.fetch('ZAPIER_HOOK_URL').freeze

  def call(user)
    add_email_to_list(user)
  rescue StandardError => e
    Raven.capture_exception(e)
    nil
  end

  private

  def add_email_to_list(user)
    response = Faraday.post(HOOK_URL, add_email_request_body(user.email, user.name))
    body = JSON.parse(response.body)

    raise StandardError, body["errors"] unless response.status == 200

    body
  rescue StandardError => e
    Raven.capture_exception(e)
    nil
  end

  def add_email_request_body(email, name)
    {
        email: email,
        name: name.split.first.capitalize
    }
  end

end
