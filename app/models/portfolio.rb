class Portfolio < ApplicationRecord
  belongs_to :user
  has_many :assets, dependent: :destroy

  validates :strategy, :benchmark, presence: true

  enum strategy: %i[fixed]
  enum benchmark: %i[^GSPC ^DJI ^IXIC ^RUT]

  BENCHMARK_NAMES = {
    '^GSPC': 'S&P 500 Index',
    '^DJI': 'Dow Jones Industrial Average',
    '^IXIC': 'Nasdaq Composite Index',
    '^RUT': 'Russell 2000 Index'
  }.freeze

  def benchmark_name
    BENCHMARK_NAMES[benchmark.to_sym]
  end

  def total_allocation
    @total_allocation = assets.sum(:allocation)
  end

  def normalized_allocations?
    puts "normalized_allocations? #{total_allocation} #{total_allocation == 1}"
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
    new_allocations = get_smart_allocations
    batch_update_allocations!(new_allocations)
  end

  def allocations_are_smart?
    assets_str = assets.map(&:ticker).sort.join('_')
    start_date = backtest_start_date || '2021-01-01'
    cache_key = "smart_allocations_#{strategy}_#{assets_str}_#{start_date}"
    return false unless Rails.cache.exist?(cache_key)

    smart_allocations = get_smart_allocations
    current_allocations = Hash[assets.map { |a| [a.ticker, a.allocation] }]
    smart_allocations == current_allocations
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

  def get_smart_allocations
    # move to service
    sorted_assets = assets.map(&:ticker).sort
    assets_str = sorted_assets.join('_')
    start_date = backtest_start_date || '2021-01-01'
    cache_key = "smart_allocations_#{strategy}_#{assets_str}_#{start_date}"
    expires_in = Utilities::Time.seconds_to_midnight_utc.seconds
    allocations = Rails.cache.fetch(cache_key, expires_in: expires_in) do
      client = FinancialDataApiClient.new
      symbols = assets.map { |a| "#{a.ticker}/USDT" }.join(',')
      allocations_result = client.smart_allocations(symbols, start_date, strategy)
      return if allocations_result.failure?

      allocations_result.data
    end

    allocations[risk_level.to_s].transform_keys { |k| k.gsub('/USDT', '') }
  end
end
