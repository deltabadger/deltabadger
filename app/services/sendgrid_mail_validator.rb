class SendgridMailValidator < BaseService

  EMAIL_VALIDATION_URL = 'https://api.sendgrid.com/v3/validations/email'.freeze

  def call(email)
    response = JSON.parse(Faraday.post(EMAIL_VALIDATION_URL, request_body(email).to_json, headers).body)
    return Result::Failure.new('Invalid Emailll') unless is_email_valid?(response)

    Result::Success.new
  end

  def get_suggestion(email)
    response = JSON.parse(Faraday.post(EMAIL_VALIDATION_URL, request_body(email).to_json, headers).body)

    suggestion(response)
  end

  private

  def headers
    api_key = ENV.fetch('SENDGRID_VALIDATION_API_KEY')

    {
      'Authorization' => "Bearer #{api_key}",
      'Content-Type' => 'application/json'
    }
  end

  def request_body(email)
    {
      'email' => email
    }
  end

  def is_email_valid?(response)
    result = response.fetch('result')
    verdict = result.fetch('verdict', 'Invalid').to_s

    verdict != 'Invalid'
  end

  def suggestion(response)
    result = response.fetch('result')
    local = result.fetch('local')
    suggestion = result.fetch('suggestion', nil)
    return suggestion if suggestion.nil?

    local.to_s + '@' + suggestion
  end
end
