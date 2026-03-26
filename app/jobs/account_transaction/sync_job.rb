class AccountTransaction::SyncJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: ->(api_key) { "account_sync_#{api_key.exchange.name_id}" }

  def perform(api_key)
    exchange_name = api_key.exchange.name
    user_id = api_key.user_id

    AccountTransactionSync.new(api_key).sync! do |percent|
      broadcast_progress(user_id, exchange_name, percent)
    end

    sleep 0.5 # Allow last progress broadcast to be delivered before replacing
    broadcast_done(user_id)
  end

  private

  def broadcast_progress(user_id, exchange_name, percent)
    Turbo::StreamsChannel.broadcast_replace_to(
      "user_#{user_id}", :sync,
      target: 'sync-progress',
      partial: 'tracker/sync_progress',
      locals: { exchange_name: exchange_name, percent: percent }
    )
  end

  def broadcast_done(user_id)
    Turbo::StreamsChannel.broadcast_remove_to(
      "user_#{user_id}", :sync,
      target: 'sync-progress'
    )
    Turbo::StreamsChannel.broadcast_refresh_to(
      "user_#{user_id}", :sync
    )
  end
end
