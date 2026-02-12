module ApplicationCable
  class Connection < ActionCable::Connection::Base
  end
end

# Silence "Unable to find subscription" errors on stale unsubscribes.
# ActionCable's execute_command rescues the error but always logs it
# as an error regardless of rescue_from handlers, so we patch remove
# to return early instead of raising.
ActionCable::Connection::Subscriptions.prepend(Module.new do
  def remove(data)
    if subscription = @subscriptions[data["identifier"]]
      remove_subscription(subscription)
    else
      logger.debug "[ActionCable] Ignoring stale unsubscribe: #{data['identifier']}"
    end
  end
end)
