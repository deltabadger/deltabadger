class SendgridMailValidator < BaseService

  EMAIL_VALIDATION_URL = 'https://api.sendgrid.com/v3/validations/email'.freeze
  INVALID = 'Invalid'.freeze

  def call(email)
    response = JSON.parse(Faraday.post(EMAIL_VALIDATION_URL, request_body(email).to_json, headers).body)
    return Result::Failure.new('Invalid Email') unless is_email_valid?(response)

    Result::Success.new
  rescue StandardError => e
    Raven.capture_exception(e)
    Result::Success.new
  end

  def get_suggestion(email)
    response = JSON.parse(Faraday.post(EMAIL_VALIDATION_URL, request_body(email).to_json, headers).body)

    suggestion(response)
  rescue StandardError => e
    Raven.capture_exception(e)
    nil
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
    result = response.fetch('result', nil?)
    return false if result.nil?

    verdict = result.fetch('verdict', INVALID).to_s
    verdict != INVALID
  end

  def suggestion(response)
    result = response.fetch('result', nil)
    return nil if result.nil?

    local = result.fetch('local', nil)
    suggestion = result.fetch('suggestion', nil)
    return suggestion if suggestion.nil?

    local.to_s + '@' + suggestion
  end
end
