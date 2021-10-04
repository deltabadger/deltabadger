class SendgridMailValidator < BaseService
  EMAIL_VALIDATION_URL = 'https://api.sendgrid.com/v3/validations/email'.freeze
  INVALID = 'Invalid'.freeze
  CACHE_KEY_BASE = 'sendgrid_response_cache_key'.freeze

  def call(email)
    response = get_response(email)

    return Result::Failure.new('Invalid Email') unless email_valid?(response)

    Result::Success.new
  rescue StandardError => e
    Raven.capture_exception(e)
    Result::Success.new
  end

  def get_suggestion(email)
    response = get_response(email)
    suggestion(response)
  rescue StandardError => e
    Raven.capture_exception(e)
    nil
  end

  private

  def get_response(email)
    cache_key = "#{CACHE_KEY_BASE}_#{email}"
    if Rails.cache.exist?(cache_key)
      response = Rails.cache.read(cache_key)
    else
      response = JSON.parse(Faraday.post(EMAIL_VALIDATION_URL, request_body(email).to_json, headers).body)
      Rails.cache.write(cache_key, response, expires_in: 24.hour)
    end

    response
  end

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

  def email_valid?(response)
    result = response.fetch('result', nil)
    return true if result.nil?

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
