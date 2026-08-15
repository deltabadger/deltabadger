class AccountTransaction::SyncJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: ->(api_key) { "account_sync_#{api_key.exchange.name_id}" }, on_conflict: :discard

  def perform(api_key)
    result = AccountTransactionSync.new(api_key).sync!
    api_key.record_sync_error!(Array(result.errors).first.to_s) if result.failure?
    TransferMatcher.run!(api_key.user)

    sleep 0.5
    broadcast_done(api_key.user_id)
  rescue StandardError => e
    api_key.record_sync_error!(e)
    broadcast_done(api_key.user_id)
    raise e
  end

  private

  def broadcast_done(user_id)
    Turbo::StreamsChannel.broadcast_remove_to(
      "user_#{user_id}", :sync,
      target: 'sync-progress'
    )
  end
end
