require 'utilities/time'

class Portfolio < ApplicationRecord
  belongs_to :user
  has_many :assets, dependent: :destroy

  validates :strategy, :benchmark, :risk_level, presence: true

  enum strategy: %i[fixed]
  enum benchmark: %i[^GSPC ^DJI ^IXIC ^RUT]
  enum risk_level: %i[conservative moderate_conservative balanced moderate_aggressive aggressive]

  BENCHMARK_NAMES = {
    '^GSPC': 'S&P 500 Index',
    '^DJI': 'Dow Jones Industrial Average',
    '^IXIC': 'Nasdaq Composite Index',
    '^RUT': 'Russell 2000 Index'
  }.freeze

  def self.humanized_risk_levels
    risk_levels.keys.map { |key| key.to_s.humanize }
  end

  def benchmark_name
    BENCHMARK_NAMES[benchmark.to_sym]
  end

  def total_allocation
    @total_allocation = assets.map{ |a| a.allocation * 10_000 }.sum / 10_000.0
  end

  def normalized_allocations?
    total_allocation == 1
  end

  def normalize_allocations!
    return if normalized_allocations?

    if total_allocation.zero?
      equal_allocation = (1.0 / assets.size).round(4)
      new_allocations = Hash[assets.map { |a| [a.ticker, equal_allocation] }]
    else
      new_allocations = Hash[assets.map { |a| [a.ticker, (a.allocation / total_allocation).round(4)] }]
    end
    new_allocations = correct_normalized_allocations(new_allocations)
    batch_update_allocations!(new_allocations)
  end

  def set_smart_allocations!
    all_smart_allocations = get_smart_allocations
    new_allocations = all_smart_allocations[Portfolio.risk_levels[risk_level].to_s]
    batch_update_allocations!(new_allocations)
  end

  def allocations_are_smart?
    return false unless Rails.cache.exist?(smart_allocations_cache_key)

    all_smart_allocations = get_smart_allocations
    current_allocations = Hash[assets.map { |a| [a.ticker, a.allocation] }]
    all_smart_allocations[Portfolio.risk_levels[risk_level].to_s] == current_allocations
  end

  def get_smart_allocations
    # move to service
    expires_in = Utilities::Time.seconds_to_midnight_utc.seconds
    allocations = Rails.cache.fetch(smart_allocations_cache_key, expires_in: expires_in) do
      client = FinancialDataApiClient.new
      symbols = assets.map { |a| a.category == 'crypto' ? "#{a.ticker}/USDT" : a.ticker }.join(',')
      sources = assets.map { |a| a.category == 'crypto' ? 'binance' : 'yfinance' }.join(',')
      allocations_result = client.smart_allocations(symbols, sources, backtest_start_date, strategy)
      return if allocations_result.failure?

      allocations_result.data
    end

    allocations.transform_values { |r| r.transform_keys { |s| s.gsub('/USDT', '') } }
  end

  def backtest
    return if backtest_start_date.blank? || !normalized_allocations?

    expires_in = Utilities::Time.seconds_to_midnight_utc.seconds
    Rails.cache.fetch(backtest_cache_key, expires_in: expires_in) do
      client = FinancialDataApiClient.new
      symbols = assets.map { |a| a.category == 'crypto' ? "#{a.ticker}/USDT" : a.ticker }.join(',')
      sources = assets.map { |a| a.category == 'crypto' ? 'binance' : 'yfinance' }.join(',')
      allocations = assets.map(&:allocation).join(',')
      metrics_result = client.metrics(symbols, sources, allocations, benchmark, backtest_start_date, strategy)
      return if metrics_result.failure?

      metrics_result.data
    end

    # backtest['metrics']['expectedReturn'].round(2)
    # backtest['metrics']['volatility'].round(2)
    # backtest['metrics']['alpha'].round(2)
    # backtest['metrics']['beta'].round(2)
    # backtest['metrics']['sharpeRatio'].round(2)
    # backtest['metrics']['sortinoRatio'].round(2)
    # backtest['metrics']['treynorRatio'].round(2)
    # backtest['metrics']['rSquared'].round(2)
    # backtest['metrics']['valueAtRisk'].round(2)
    # backtest['metrics']['conditionalValueAtRisk'].round(2)
    # backtest['metrics']['omegaRatio'].round(2)
    # backtest['metrics']['calmarRatio'].round(2)
    # backtest['metrics']['ulcerIndex'].round(2)
    # backtest['metrics']['maxDrawdown'].round(2)
    # backtest['metrics']['cagr'].round(2)
    # backtest['metrics']['informationRatio'].round(2)
  end

  private

  def smart_allocations_cache_key
    assets_str = assets.map(&:ticker).sort.join('_')
    start_date = backtest_start_date || '2021-01-01'
    "smart_allocations_#{strategy}_#{assets_str}_#{start_date}"
  end

  def backtest_cache_key
    assets_str = assets.map(&:ticker).sort.join('_')
    allocations_str = assets.map(&:allocation).join('_')
    "simulation_#{strategy}_#{assets_str}_#{allocations_str}_#{benchmark}_#{backtest_start_date}"
  end

  def batch_update_allocations!(new_allocations)
    raise ActiveRecord::RecordInvalid, 'Invalid number of allocations' if assets.size != new_allocations.size

    ActiveRecord::Base.transaction do
      assets.each do |asset|
        unless asset.update(allocation: new_allocations[asset.ticker])
          raise ActiveRecord::RecordInvalid, 'Invalid allocation value'
        end
      end
    end
  end

  def correct_normalized_allocations(normalized_allocations)
    recalculated_total = normalized_allocations.values.sum
    correction = (1.0 - recalculated_total).round(4)
    return normalized_allocations if correction.zero?

    normalized_allocations.each do |asset, new_allocation|
      adjusted_allocation = (new_allocation + correction).round(4)
      if adjusted_allocation >= 0 && adjusted_allocation <= 1
        normalized_allocations[asset] = adjusted_allocation
        break
      end
    end
    normalized_allocations
  end
end
