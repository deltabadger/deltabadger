class AccountTransaction::SyncAllJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: -> { name }, on_conflict: :discard, duration: 1.hour

  def perform
    ApiKey.where(key_type: :trading, status: :correct).find_each.with_index do |api_key, i|
      AccountTransaction::SyncJob.set(wait: i * 30.seconds).perform_later(api_key)
    end
  end
end
