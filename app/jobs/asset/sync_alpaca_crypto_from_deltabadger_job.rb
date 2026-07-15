class Asset::SyncAlpacaCryptoFromDeltabadgerJob < ApplicationJob
  queue_as :default
  limits_concurrency to: 1, key: 'sync_alpaca_crypto_from_deltabadger', on_conflict: :discard, duration: 1.hour

  retry_on Client::TransientNetworkError, wait: :polynomially_longer, attempts: 5
  retry_on Client::RateLimitedError, wait: :polynomially_longer, attempts: 5

  def perform
    return unless MarketDataSettings.deltabadger?

    result = MarketData.sync_alpaca_crypto_listings_from_deltabadger!
    Rails.logger.warn "[SyncAlpacaCrypto] sync failed: #{result.errors.to_sentence}" if result.failure?
  end
end
