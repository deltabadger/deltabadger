class AccountBalance::SyncAllJob < ApplicationJob
  queue_as :low_priority

  def perform
    ApiKey.reading(ApiKey.includes(:exchange))
          .group_by(&:user_id).each do |user_id, api_keys|
      AccountBalance::SyncJob.perform_later(user_id, api_keys.map(&:id))
    end
  end
end
