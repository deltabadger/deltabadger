class AccountTransaction::SyncTrackerJob < ApplicationJob
  include ApiKeyFailureHandling

  queue_as :low_priority
  limits_concurrency to: 1, key: ->(user_id, *) { "sync_tracker_#{user_id}" }, on_conflict: :discard

  def perform(user_id, api_key_ids)
    api_keys = ApiKey.where(id: api_key_ids).includes(:exchange)

    failed = api_keys.filter_map { |api_key| sync_exchange(user_id, api_key) }
    TransferMatcher.run!(User.find(user_id))

    sleep 0.5
    broadcast_done(user_id)
    # Broadcast unconditionally so a clean sync clears any stale warning.
    Turbo::StreamsChannel.broadcast_replace_to(
      "user_#{user_id}", :sync,
      target: 'sync-warnings',
      partial: 'tracker/sync_warning',
      locals: { exchanges: failed }
    )
  rescue StandardError => e
    broadcast_done(user_id)
    raise e
  end

  private

  def sync_exchange(user_id, api_key)
    exchange_name = api_key.exchange.name

    result = AccountTransactionSync.new(api_key).sync! do |percent|
      broadcast_progress(user_id, exchange_name, percent)
    end
    return if result.success?

    handle_api_key_failure(api_key, result, capability: :transactions)
    api_key.record_sync_error!(Array(result.errors).first.to_s)
    exchange_name
  rescue StandardError => e
    Rails.logger.error("[SyncTracker] #{api_key.exchange.name} failed: #{e.message}")
    api_key.record_sync_error!(e)
    exchange_name
  end

  def broadcast_done(user_id)
    Turbo::StreamsChannel.broadcast_remove_to(
      "user_#{user_id}", :sync,
      target: 'sync-progress'
    )
    Turbo::StreamsChannel.broadcast_refresh_to("user_#{user_id}", :sync)
  end

  def broadcast_progress(user_id, exchange_name, percent)
    Turbo::StreamsChannel.broadcast_replace_to(
      "user_#{user_id}", :sync,
      target: 'sync-progress',
      partial: 'tracker/sync_progress',
      locals: { exchange_name: exchange_name, percent: percent }
    )
  end
end
