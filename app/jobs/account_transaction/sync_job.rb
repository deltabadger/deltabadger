class AccountTransaction::SyncJob < ApplicationJob
  queue_as :low_priority
  # Keyed on the USER as well as the venue. It was venue-only, and with on_conflict: :discard that
  # did not delay a second user's sync — it threw it away, so on a multi-user instance one user's
  # Binance sync silently stopped every other user's ledger from catching up.
  #
  # Inlined rather than calling a helper: Solid Queue instance_execs this lambda on the job, so a
  # bare class-method call inside it raises NoMethodError at enqueue time.
  limits_concurrency to: 1, on_conflict: :discard,
                     key: ->(api_key) { "account_sync_#{api_key.user_id}_#{api_key.exchange.name_id}" }

  def perform(api_key)
    result = AccountTransactionSync.new(api_key).sync!
    api_key.record_sync_error!(Array(result.errors).first.to_s) if result.failure?
    TransferMatcher.run!(api_key.user)

    sleep 0.5
    # New rows mean a new ledger, and the tracker reads it from the cache — so the figures follow
    # the sync rather than waiting for the next visit to notice they are stale.
    Tracker::LedgerJob.perform_later(api_key.user_id)
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
