class Bot::WarmMetricsCachesJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: 'WarmMetricsCachesJob', on_conflict: :discard, duration: 5.minutes

  # Bot types that expose performance metrics (the global-PnL / index inclusion set).
  MEASURABLE_TYPES = %w[
    Bots::DcaSingleAsset
    Bots::DcaDualAsset
    Bots::DcaIndex
    Bots::Signal
  ].freeze

  # Keeps the current-price caches hot so the /bots index (cache-only global PnL) and the
  # per-bot PnL broadcasts read warm caches instead of doing live exchange roundtrips.
  # Price-only on purpose: the heavier candle path stays lazy (warmed on actual view).
  def perform
    fiat_currencies = Set.new

    measurable_bots_with_history.each do |bot|
      warm_prices(bot)

      currency = bot.quote_asset&.symbol
      fiat_currencies << currency.upcase if currency.present? && currency.upcase != 'USD'
    end

    warm_fx_rates(fiat_currencies)
  end

  private

  def measurable_bots_with_history
    Bot.not_deleted.where(type: MEASURABLE_TYPES).select do |bot|
      bot.transactions.submitted.exists?
    end
  end

  def warm_prices(bot)
    bot.metrics_with_current_prices(force: true)
  rescue StandardError => e
    Rails.logger.error "[WarmMetricsCaches] price warm failed for bot #{bot.id}: #{e.message}"
  end

  def warm_fx_rates(currencies)
    currencies.each do |currency|
      Utilities::Currency.exchange_rate(from: currency, to: 'USD')
    rescue StandardError => e
      Rails.logger.error "[WarmMetricsCaches] FX warm failed for #{currency}: #{e.message}"
    end
  end
end
