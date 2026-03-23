class AccountTransaction::SyncJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: ->(api_key) { "account_sync_#{api_key.exchange.name_id}" }

  def perform(api_key)
    AccountTransactionSync.new(api_key).sync!
  end
end
