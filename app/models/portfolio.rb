require 'utilities/time'

class Portfolio < ApplicationRecord
  belongs_to :user
  has_many :assets, dependent: :destroy

  validates :strategy, :benchmark, :risk_level, presence: true

  enum strategy: %i[fixed]
  enum benchmark: %i[^GSPC ^DJI ^IXIC ^RUT BTC/USDT]
  enum risk_level: %i[conservative moderate_conservative balanced moderate_aggressive aggressive]

  BENCHMARK_NAMES = {
    '^GSPC': { source: 'yfinance', name: 'S&P 500 Index' },
    '^DJI': { source: 'yfinance', name: 'Dow Jones Industrial Average' },
    '^IXIC': { source: 'yfinance', name: 'Nasdaq Composite Index' },
    '^RUT': { source: 'yfinance', name: 'Russell 2000 Index' },
    'BTC/USDT': { source: 'binance', name: 'Bitcoin' }
  }.freeze
  RISK_FREE_RATES = {
    '^IRX': { shortname: '13W', name: '13 Week US Treasury Bill Yield' },
    '^FVX': { shortname: '5Y', name: '5 Year US Treasury Note Yield' },
    '^TNX': { shortname: '10Y', name: '10 Year US Treasury Note Yield' }
  }.freeze

  def self.humanized_risk_levels
    risk_levels.keys.map { |key| key.to_s.humanize }
  end

  def self.benchmark_select_options
    benchmarks.keys.map { |key| [BENCHMARK_NAMES[key.to_sym][:name], key] }
  end

  def smart_allocations
    @smart_allocations ||= begin
      smart_allocations_result = PortfolioAnalyzerManager::SmartAllocationsGetter.call(self)
      if smart_allocations_result.failure?
        self.class.risk_levels.keys.map { |_| [] }
      else
        smart_allocations_result.data
      end
    end
  end

  def benchmark_name
    BENCHMARK_NAMES[benchmark.to_sym][:name]
  end

  def benchmark_source
    BENCHMARK_NAMES[benchmark.to_sym][:source]
  end

  def total_allocation
    assets.map(&:allocation).sum.round(4)
  end

  def allocations_are_normalized?
    total_allocation == 1
  end

  def normalize_allocations!
    return if allocations_are_normalized?

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
    return if allocations_are_smart? || assets.empty?

    new_allocations = smart_allocations[Portfolio.risk_levels[risk_level].to_i]
    return if new_allocations.empty?

    batch_update_allocations!(new_allocations)
  end

  def allocations_are_smart?
    return false unless Rails.cache.exist?(smart_allocations_cache_key)

    current_allocations = Hash[assets.map { |a| [a.ticker, a.allocation] }]
    smart_allocations[Portfolio.risk_levels[risk_level].to_i] == current_allocations
  end

  def backtest
    return if backtest_start_date.blank? || !allocations_are_normalized?

    backtest_result = PortfolioAnalyzerManager::BacktestResultsGetter.call(self)
    return if backtest_result.failure?

    backtest_result.data
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

  def backtest_api_error
    cached_result = Rails.cache.read(backtest_cache_key)
    return unless cached_result.present?

    PortfolioAnalyzerManager::FinancialDataApiErrorParser.call(cached_result)
  end

  def ordered_assets
    @ordered_assets ||= assets.order(:id)
  end

  def active_assets
    @active_assets ||= if smart_allocation_on? && !smart_allocations[Portfolio.risk_levels[risk_level].to_i].empty?
                         ordered_assets.select do |asset|
                           smart_allocations.map { |sa| sa[asset.ticker] }.sum.positive?
                         end
                       else
                         ordered_assets
                       end
  end

  def idle_assets
    @idle_assets ||= ordered_assets - active_assets
  end

  def max_assets_reached?
    assets.size >= case user.subscription_name
                   when 'legendary_badger', 'hodler'
                     20
                   when 'investor'
                     4
                   else
                     4
                   end
  end

  def reset_memoized_assets
    @ordered_assets = nil
    @active_assets = nil
    @idle_assets = nil
  end

  def chatgpt_prompt
    text = ''
    text += 'This is my portfolio: '
    text += 'Assets:'
    assets.each do |asset|
      text += " #{asset.ticker} #{(asset.allocation * 100).round(2)}%"
    end
    text += '. '
    text += "Benchmark: #{benchmark_name}. "
    text += "Risk-free rate: #{(risk_free_rate * 100).round(2)}%. "
    text += "Metrics for time since #{backtest_start_date} to #{1.day.ago.to_date}: "
    text += "Portfolio performance +#{backtest['metrics']['totalReturn'].round(2)}%, "
    text += "Benchmark performance +#{backtest['metrics']['benchmarkTotalReturn'].round(2)}%, "
    text += "Expected Return #{backtest['metrics']['expectedReturn'].round(2)}%, "
    text += "CAGR #{backtest['metrics']['cagr'].round(2)}%, "
    text += "Volatility #{backtest['metrics']['volatility'].round(2)}%, "
    text += "Max. Drawdown #{backtest['metrics']['maxDrawdown'].round(2)}%, "
    text += "Calmar Ratio #{backtest['metrics']['calmarRatio'].round(2)}, "
    text += "VaR #{backtest['metrics']['valueAtRisk'].round(2)}%, "
    text += "CVaR #{backtest['metrics']['conditionalValueAtRisk'].round(2)}%, "
    text += "Sharpe Ratio #{backtest['metrics']['sharpeRatio'].round(2)}, "
    text += "Sortino Ratio #{backtest['metrics']['sortinoRatio'].round(2)}, "
    text += "Treynor Ratio #{backtest['metrics']['treynorRatio'].round(2)}, "
    text += "Omega Ratio #{backtest['metrics']['omegaRatio'].round(2)}, "
    text += "Alpha #{backtest['metrics']['alpha'].round(2)}, "
    text += "Beta #{backtest['metrics']['beta'].round(2)}, "
    text += "R-squared #{backtest['metrics']['rSquared'].round(2)}, "
    text += "Information Ratio #{backtest['metrics']['informationRatio'].round(2)}"
    text += ', Diversification 80%.'
    text
  end

  def get_risk_free_rate(key)
    return if key.blank? || !RISK_FREE_RATES.include?(key.to_sym)

    expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
    cache_key = "risk_free_rate_#{key}"
    time_series_result_data = Rails.cache.fetch(cache_key, expires_in: expires_in) do
      client = FinancialDataApiClient.new
      time_series_result = client.time_series(
        symbol: key,
        timeframe: '1d',
        source: 'yfinance',
        limit: 1
      )
      return if time_series_result.failure?

      time_series_result.data
    end
    (time_series_result_data[0][4] / 100).round(4)
  end

  def smart_allocations_cache_key
    assets_str = ordered_assets.map(&:ticker).sort.join('_')
    "smart_allocations_#{strategy}_#{assets_str}_#{benchmark}_#{backtest_start_date}_#{risk_free_rate}"
  end

  def backtest_cache_key
    assets_str = ordered_assets.map(&:ticker).sort.join('_')
    allocations_str = ordered_assets.map(&:allocation).join('_')
    "simulation_#{strategy}_#{assets_str}_#{allocations_str}_#{benchmark}_#{backtest_start_date}_#{risk_free_rate}"
  end

  private

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
