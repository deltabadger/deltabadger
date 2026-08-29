class Bot::WarmMetricsCachesJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: 'WarmMetricsCachesJob', on_conflict: :discard, duration: 5.minutes

  # Bot types that expose performance metrics (the global-PnL / index inclusion set).
  MEASURABLE_TYPES = %w[
    Bots::DcaSingleAsset
    Bots::DcaIndex
    Bots::DcaMultiAsset
    Bots::Signal
  ].freeze

  # Warming is a page-load optimisation, so it follows actual use: every signed-in request
  # opens this window, and outside it the job does nothing. Ungated it is a live price call
  # per exchange every five minutes, around the clock, to fill a cache no one will read.
  # Closing the window breaks nothing — a cold metrics cache renders a placeholder that the
  # page fills live on connect, exactly as the first visit after an idle stretch does.
  ACTIVITY_KEY = 'recent_web_activity'.freeze
  ACTIVITY_WINDOW = 30.minutes

  # Called from every signed-in request. A plain cache write, so it also self-disables where
  # caching is off — there is nothing to warm there either.
  def self.mark_activity!
    Rails.cache.write(ACTIVITY_KEY, true, expires_in: ACTIVITY_WINDOW)
  end

  # Keeps the current-price caches hot so the /bots index (cache-only global PnL) and the
  # per-bot PnL broadcasts read warm caches instead of doing live exchange roundtrips.
  # Price-only on purpose: the heavier candle path stays lazy (warmed on actual view).
  def perform
    return unless Rails.cache.exist?(ACTIVITY_KEY)

    fiat_currencies = Set.new

    measurable_bots_with_history.each do |bot|
      warm_prices(bot)

      currency = bot.quote_asset&.symbol
      fiat_currencies << currency.upcase if currency.present? && currency.upcase != 'USD'
    end

    warm_fx_rates(fiat_currencies)
  end

  private

  # Correlated EXISTS, not `id: Transaction.submitted.select(:bot_id)`: the transactions index
  # leads with bot_id, so only the correlated form seeks it (and stops at the first match)
  # instead of scanning every submitted row in the account's whole history.
  def measurable_bots_with_history
    history = Transaction.submitted.where('transactions.bot_id = bots.id')
    Bot.not_deleted.where(type: MEASURABLE_TYPES).where(history.arel.exists)
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
