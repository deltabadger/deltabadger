class AccountTransaction::SyncTrackerJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: ->(user_id, *) { "sync_tracker_#{user_id}" }, on_conflict: :discard

  def perform(user_id, api_key_ids)
    api_keys = ApiKey.where(id: api_key_ids).includes(:exchange)

    api_keys.each do |api_key|
      sync_exchange(user_id, api_key)
    end

    sleep 0.5
    broadcast_done(user_id)
  rescue StandardError => e
    broadcast_done(user_id)
    raise e
  end

  private

  def sync_exchange(user_id, api_key)
    exchange_name = api_key.exchange.name

    AccountTransactionSync.new(api_key).sync! do |percent|
      broadcast_progress(user_id, exchange_name, percent)
    end
  rescue StandardError => e
    Rails.logger.error("[SyncTracker] #{api_key.exchange.name} failed: #{e.message}")
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
