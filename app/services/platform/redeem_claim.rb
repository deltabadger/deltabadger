module Platform
  class RedeemClaim < BaseService
    INVALID_CLAIM_MESSAGE = 'That claim code is invalid or has expired.'.freeze
    INVALID_MARKET_DATA_MESSAGE = 'Deltabadger returned incomplete market data configuration. Please try again.'.freeze

    def call(code:)
      result = Clients::Launchpad.new.claim(code)
      return Result::Failure.new(INVALID_CLAIM_MESSAGE) if unauthorized?(result)
      return result if result.failure?

      payload = result.data
      market_data = payload[:market_data] if payload.is_a?(Hash)
      unless market_data.is_a?(Hash) && market_data[:url].present? && market_data[:token].present?
        return Result::Failure.new(INVALID_MARKET_DATA_MESSAGE)
      end

      ApplicationRecord.transaction do
        AppConfig.market_data_url = market_data[:url]
        AppConfig.market_data_token = market_data[:token]
        AppConfig.market_data_provider = MarketDataSettings::PROVIDER_DELTABADGER
        (payload[:proxies] || {}).each do |key, value|
          AppConfig.set("proxy_#{key.to_s.downcase}", value)
        end
        AppConfig.set('platform_connected_at', Time.current.iso8601)
        AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_PENDING
      end

      Setup::SeedAndSyncJob.perform_later
      Result::Success.new(payload[:identity])
    end

    private

    def unauthorized?(result)
      result.failure? && result.data.is_a?(Hash) && result.data[:status] == 401
    end
  end
end
