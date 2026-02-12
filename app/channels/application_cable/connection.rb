module ApplicationCable
  class Connection < ActionCable::Connection::Base
    rescue_from RuntimeError, with: :handle_runtime_error

    private

    def handle_runtime_error(error)
      raise error unless error.message.start_with?('Unable to find subscription')

      logger.debug "[ActionCable] Ignoring stale unsubscribe: #{error.message}"
    end
  end
end
