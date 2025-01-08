require 'utilities/time'

class Portfolio < ApplicationRecord
  belongs_to :user
  has_many :assets, dependent: :destroy

  validates :strategy, :benchmark, :risk_level, presence: true

  enum strategy: %i[fixed]
  enum benchmark: %i[65951 1713 65437 65775 65992 37818 61914 61885 51788 37549]
  enum risk_level: %i[conservative moderate_conservative balanced moderate_aggressive aggressive]

  MAX_ASSETS = {
    limited: 4,
    unlimited: 100
  }.freeze
  BENCHMARK_NAMES = {
    '65951': { name: 'S&P 500 Index' },
    '1713': { name: 'Bitcoin' },
    '65437': { name: 'Gold' },
    '65775': { name: 'Dow Jones Industrial Average' },
    '65992': { name: 'Nasdaq Composite Index' },
    '37818': { name: 'Russell 2000 Index' },
    '61914': { name: 'Vanguard Total Stock Market Index' },
    '61885': { name: 'Vanguard Total World Stock Index' },
    '51788': { name: 'Invesco QQQ Trust' },
    '37549': { name: 'iShares U.S. Aerospace & Defense ETF' }
  }.freeze
  RISK_FREE_RATES = {
    '66411': { shortname: '1Y', name: '1 Year US Treasury' },
    '66416': { shortname: '5Y', name: '5 Year US Treasury' },
    '66409': { shortname: '10Y', name: '10 Year US Treasury' }
  }.freeze

  def self.humanized_risk_levels
    risk_levels.keys.map { |key| key.to_s.humanize }
  end

  def self.benchmark_select_options
    benchmarks.keys.map { |key| [BENCHMARK_NAMES[key.to_sym][:name], key] }
  end

  def limited?
    user.subscription.free? || user.subscription.basic?
  end

  def compare_to_select_options
    user.portfolios.includes(:assets).all.map do |portfolio|
      if !portfolio.id.in?([id] + compare_to) && portfolio.assets.present? && portfolio.allocations_are_normalized?
        [portfolio.label, portfolio.id]
      end
    end.compact
  end

  def compare_to_selected_options
    compare_to.map do |portfolio_id|
      portfolio = user.portfolios.find(portfolio_id)
      [portfolio.label, portfolio_id]
    end.compact
  end

  def smart_allocations
    @smart_allocations ||= begin
      smart_allocations_result = PortfolioAnalyzerManager::SmartAllocationsGetter.call(self)
      if smart_allocations_result.failure?
        Rails.logger.error("Smart allocations error: #{smart_allocations_result.errors} for assets #{assets.map(&:api_id)}")
        # set all allocations as 0 if there is an API error
        self.class.risk_levels.keys.map { |_| assets.map { |a| [a.api_id, 0] }.to_h }
      else
        smart_allocations_result.data
      end
    end
  end

  def benchmark_name
    BENCHMARK_NAMES[benchmark.to_sym][:name]
  end

  def total_allocation
    assets.map(&:effective_allocation).sum.round(4)
  end

  def allocations_are_normalized?
    total_allocation == 1
  end

  def normalize_allocations!
    return if allocations_are_normalized?

    if total_allocation.zero?
      equal_allocation = (1.0 / assets.size).round(4)
      new_allocations = Hash[assets.map { |a| [a.api_id, equal_allocation] }]
    else
      new_allocations = Hash[assets.map { |a| [a.api_id, (a.allocation / total_allocation).round(4)] }]
    end
    new_allocations = correct_normalized_allocations(new_allocations)
    batch_update_allocations!(new_allocations)
  end

  def backtest(custom_start_date: nil)
    return unless allocations_are_normalized?

    backtest_result = PortfolioAnalyzerManager::BacktestResultsGetter.call(self, custom_start_date: custom_start_date)
    return unless backtest_result.success?

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

  def risk_level_int
    Portfolio.risk_levels[risk_level]
  end

  def ordered_assets
    @ordered_assets ||= assets.order(:id)
  end

  def active_assets
    @active_assets ||= if assets.present? && smart_allocation_on? && !smart_allocations[risk_level_int].empty?
                         ordered_assets.select do |asset|
                           smart_allocations.map { |sa| sa[asset.api_id] }.sum.positive?
                         end
                       else
                         ordered_assets
                       end
  end

  def idle_assets
    @idle_assets ||= ordered_assets - active_assets
  end

  def max_assets_reached?
    assets.size >= if user.subscription.pro? || user.subscription.legendary?
                     100
                   else
                     4
                   end
  end

  def reset_memoized_assets
    @ordered_assets = nil
    @active_assets = nil
    @idle_assets = nil
  end

  def get_risk_free_rate(key)
    return Result::Failure.new(I18n.t('errors.invalid_risk_free_rate_key')) if key.blank? || !RISK_FREE_RATES.key?(key.to_sym)

    expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
    cache_key = "risk_free_rate_#{key}"
    time_series_result = Rails.cache.fetch(cache_key, expires_in: expires_in) do
      client = FinancialDataApiClient.new
      time_series_result = client.time_series(
        symbol: key,
        timeframe: '1d',
        limit: 1
      )
      risk_free_rate_name = RISK_FREE_RATES[key.to_sym][:name]
      error_message = I18n.t('errors.unable_to_get_risk_free_rate', risk_free_rate_name: risk_free_rate_name)
      return Result::Failure.new(error_message) if time_series_result.failure?

      time_series_result
    end
    Result::Success.new((time_series_result.data[0][4] / 100).round(4))
  end

  def smart_allocations_cache_key
    assets_str = ordered_assets.map(&:api_id).sort.join('_')
    "smart_allocations_#{strategy}_#{assets_str}_#{benchmark}_#{backtest_start_date}_#{risk_free_rate}"
  end

  def backtest_cache_key(custom_start_date: nil)
    assets_str = ordered_assets.map(&:api_id).sort.join('_')
    allocations_str = ordered_assets.map(&:effective_allocation).join('_')
    start_date = custom_start_date || backtest_start_date
    "simulation_#{strategy}_#{assets_str}_#{allocations_str}_#{benchmark}_#{start_date}_#{risk_free_rate}"
  end

  private

  def batch_update_allocations!(new_allocations)
    raise ActiveRecord::RecordInvalid, I18n.t('errors.invalid_number_of_allocations') if assets.size != new_allocations.size

    ActiveRecord::Base.transaction do
      assets.each do |asset|
        unless asset.update(allocation: new_allocations[asset.api_id])
          raise ActiveRecord::RecordInvalid, I18n.t('errors.invalid_allocation_value')
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
