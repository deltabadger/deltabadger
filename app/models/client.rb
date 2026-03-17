class Client
  OPTIONS = {
    request: {
      open_timeout: 5,   # seconds to wait for the connection to open
      read_timeout: 30,  # seconds to wait for one block to be read
      write_timeout: 10  # seconds to wait for one block to be written
    }
  }.freeze

  def with_rescue
    yield
  rescue Faraday::Error => e
    body = e.response_body.presence
    error_message = if body&.match?(/<\s*html/i)
                      "HTTP #{e.response_status || 'error'}"
                    else
                      body || e.message.presence || 'Unknown API error'
                    end
    Result::Failure.new(error_message, data: { status: e.response_status })
  rescue StandardError => e
    Result::Failure.new(e.message.presence || 'Unknown error')
  end
end
