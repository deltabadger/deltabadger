class ApplicationClient
  OPTIONS = {
    request: {
      open_timeout: 5,      # seconds to wait for the connection to open
      read_timeout: 5,      # seconds to wait for one block to be read
      write_timeout: 5      # seconds to wait for one block to be written
    }
  }.freeze

  def with_rescue
    yield
  rescue Faraday::Error => e
    Result::Failure.new(e.response_body)
  rescue StandardError => e
    Raven.capture_exception(e)
    Result::Failure.new(e.message)
  end
end
