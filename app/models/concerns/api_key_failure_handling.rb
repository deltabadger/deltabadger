module ApiKeyFailureHandling
  extend ActiveSupport::Concern

  # Handle a Result::Failure returned from a live exchange call performed with
  # an API key. If the failure looks like an invalid-key error, flip the key's
  # status to :incorrect and broadcast a flash so the UI re-renders with a
  # "broken" button pointing at the edit form.
  def handle_api_key_failure(api_key, result)
    return if result.success?

    errors = result.errors
    exchange = api_key.exchange
    message = Array(errors).first.to_s

    Rails.logger.warn("[SyncKeyFailure] #{exchange.name} api_key=#{api_key.id}: #{message}")

    api_key.update!(status: :incorrect) if exchange.invalid_key_error?(errors)

    Turbo::StreamsChannel.broadcast_append_to(
      "user_#{api_key.user_id}", :sync,
      target: 'flash',
      partial: 'tracker/sync_key_error',
      locals: { exchange_name: exchange.name, message: message }
    )
  end
end
