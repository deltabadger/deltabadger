class AccountTransaction::SyncAllJob < ApplicationJob
  queue_as :low_priority

  def perform
    ApiKey.where(key_type: :trading, status: :correct).find_each.with_index do |api_key, i|
      AccountTransaction::SyncJob.set(wait: i * 30.seconds).perform_later(api_key)
    end
  end
end
