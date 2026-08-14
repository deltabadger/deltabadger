class Clients::Launchpad < Client
  OPTIONS = {
    request: {
      open_timeout: 5,
      read_timeout: 10
    }
  }.freeze

  def initialize
    super
    @url = ENV.fetch('LAUNCHPAD_URL', 'https://deltabadger.com')
  end

  def claim(code)
    with_rescue do
      response = connection.post('api/claim', { code: code })
      Result::Success.new(response.body.deep_symbolize_keys)
    end
  end

  private

  def connection
    @connection ||= Faraday.new(url: @url, **OPTIONS) do |config|
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # Claim redemption is an interactive, idempotent lookup. Unlike trading calls, callers need a
  # Result they can display rather than a retry-signalling exception.
  def with_rescue
    super
  rescue Client::TransientNetworkError
    Result::Failure.new('Unable to reach Deltabadger. Please try again.')
  end
end
