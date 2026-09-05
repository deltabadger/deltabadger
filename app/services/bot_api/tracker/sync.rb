# frozen_string_literal: true

module BotApi
  module Tracker
    # `::Tracker::` for the job: inside BotApi::Tracker a bare `Tracker::` resolves here.
    class Sync
      def self.call(user:)
        # ApiKey.reading returns an Array, not a relation, and at most one key per venue.
        api_keys = ApiKey.reading(user.api_keys.includes(:exchange))
        if api_keys.empty?
          return Result.failure(:validation_failed, 'no_reading_keys',
                                'No exchange key can read history. Add one in Settings.')
        end

        ids = api_keys.map(&:id)
        AccountTransaction::SyncTrackerJob.perform_later(user.id, ids)
        AccountBalance::SyncJob.perform_later(user.id, ids)
        Result.success({ exchanges: api_keys.map { |key| key.exchange.name } }, status: :accepted)
      end
    end
  end
end
