class AccountBalance::SyncJob < ApplicationJob
  include ApiKeyFailureHandling

  queue_as :low_priority
  limits_concurrency to: 1, key: ->(user_id, *) { "sync_balances_#{user_id}" }, on_conflict: :discard

  def perform(user_id, api_key_ids)
    api_keys = ApiKey.reading(ApiKey.where(id: api_key_ids).includes(:exchange))
    return if api_keys.empty?

    pricing_errors = []
    api_keys.each do |api_key|
      err = sync(api_key)
      pricing_errors << err if err
    end

    # Where the portfolio history grows by one day. Here rather than on a schedule of its own: the
    # figures this records are exactly the ones the loop above just refreshed.
    PortfolioSnapshot.record!(User.find(user_id))
    broadcast_pricing_warning(user_id, pricing_errors.first) if pricing_errors.any?
    broadcast_refresh(user_id)
  end

  private

  # Returns pricing_error string if live pricing fully failed for this key, else nil.
  def sync(api_key)
    result = AccountBalance::Sync.new(api_key).sync!
    handle_api_key_failure(api_key, result, capability: :balances)
    return record_failure(api_key, Array(result.errors).first.to_s) unless result.success?

    summary = result.data
    summary.pricing_fully_failed? ? summary.pricing_error : nil
  rescue StandardError => e
    Rails.logger.error("[AccountBalance::SyncJob] #{api_key.exchange.name} failed: #{e.message}")
    record_failure(api_key, e)
  end

  # The broadcast above is the whole story only for a user sitting on the tracker with a live Turbo
  # subscription. This job also runs unattended (AccountBalance::SyncAllJob), and since a permission
  # failure no longer condemns the key, without this the balances would just quietly stop updating.
  # `last_sync_error` is the app's durable channel for that — TrackerController#index rebuilds a
  # banner from it on every page load.
  #
  # ponytail: one error field is shared with the transaction sync, which clears it on success. A key
  # that can read the ledger but not the balances (Kraken: Query Ledger Entries without Funds→Query)
  # can therefore have this warning erased by a transaction sync finishing after it — the two jobs
  # are enqueued independently. Narrow: that key also breaks the bots' own balance reads, which is a
  # far louder signal. Upgrade path if it ever bites: store the error per capability
  # (`last_balance_sync_error`) and keep ApiKey#sync_issue reading only the transaction one, since
  # the tax report's warning is a statement about transaction-history completeness.
  def record_failure(api_key, error)
    api_key.record_sync_error!(error)
    nil
  end

  def broadcast_pricing_warning(user_id, message)
    Turbo::StreamsChannel.broadcast_append_to(
      "user_#{user_id}", :sync,
      target: 'flash',
      partial: 'tracker/pricing_warning',
      locals: { message: message }
    )
  end

  def broadcast_refresh(user_id)
    Turbo::StreamsChannel.broadcast_refresh_to("user_#{user_id}", :sync)
  end
end
