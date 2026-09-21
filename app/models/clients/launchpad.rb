class Clients::Launchpad < Client
  RETRYABLE_CLAIM_MESSAGE = 'Unable to reach Deltabadger. Please try again.'.freeze
  AMBIGUOUS_CLAIM_MESSAGE = 'Deltabadger may already have used this one-time code. ' \
                            'Generate a fresh code from the dashboard and try again.'.freeze
  PRE_SEND_FAILURES = [
    'Net::OpenTimeout',
    'Errno::ECONNREFUSED',
    'SocketError',
    'Socket::ResolutionError'
  ].freeze

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
  # Result they can display rather than a retry-signalling exception. A transport failure no retry
  # can fix comes back from Client.network_failure as a Result with its exception chain instead of
  # raising, and gets the same two messages.
  def with_rescue
    result = super
    chain = result.data[:error_chain] if result.failure? && result.data.is_a?(Hash)
    chain ? claim_network_failure(Client.most_specific_cause(chain)) : result
  rescue Client::TransientNetworkError => e
    claim_network_failure(e.original_class)
  end

  def claim_network_failure(cause)
    definitely_pre_send = PRE_SEND_FAILURES.include?(cause) || cause&.start_with?('Resolv::')
    Result::Failure.new(definitely_pre_send ? RETRYABLE_CLAIM_MESSAGE : AMBIGUOUS_CLAIM_MESSAGE)
  end
end
