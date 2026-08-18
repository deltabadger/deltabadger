module ApiKeyFailureHandling
  extend ActiveSupport::Concern

  # Handle a Result::Failure returned from a live exchange call performed with an API key, and
  # tell the user something they can act on.
  #
  # `capability` names what this caller was trying to read (:transactions, :balances) — the two
  # includers both know, and a permission failure is only actionable if the message says which
  # of our features the key cannot do.
  #
  # Only an invalid-key failure flips the key to :incorrect. A missing PERMISSION must not: the
  # credentials are valid, the key is very likely still trading, and :incorrect removes it from
  # every `status: :correct`-scoped sync (tracker#sync, AccountBalance::SyncJob) — which would
  # strand a user who has just fixed the permission with no way to re-sync. The failure still
  # persists via ApiKey#last_sync_error, which TrackerController#index rebuilds into a banner on
  # every page load, so nothing goes silent.
  def handle_api_key_failure(api_key, result, capability:)
    return if result.success?

    errors = result.errors
    exchange = api_key.exchange
    message = Array(errors).first.to_s

    Rails.logger.warn("[SyncKeyFailure] #{exchange.name} api_key=#{api_key.id}: #{message}")

    reason = failure_reason(exchange, errors, status: exchange.http_status(result))
    api_key.update!(status: :incorrect) if reason == :invalid

    # Both the wrapper copy and humanize_error resolve I18n immediately, so both belong inside
    # the block: AccountBalance::SyncAllJob is a scheduled fan-out with no request locale, and
    # hoisting the humanize call would wrap localized copy around an English inner message.
    I18n.with_locale(api_key.user.locale || I18n.default_locale) do
      Turbo::StreamsChannel.broadcast_append_to(
        "user_#{api_key.user_id}", :sync,
        target: 'flash',
        partial: 'tracker/sync_key_error',
        locals: { exchange_name: exchange.name, message: exchange.humanize_error(message),
                  reason: reason, capability: capability }
      )
    end
  end

  private

  def failure_reason(exchange, errors, status: nil)
    return :permission if exchange.permission_error?(errors)
    return :invalid if exchange.invalid_key_error?(errors, status: status)
    return :transient if exchange.transient_error?(errors)

    :failed
  end
end
