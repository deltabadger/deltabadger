module Bots::Barbell::MarketcapAllocatable
  extend ActiveSupport::Concern

  included do
    store_accessor :settings,
                   :marketcap_allocated

    after_initialize :initialize_marketcap_allocatable_settings

    validates :marketcap_allocated, inclusion: { in: [true, false] }
  end

  def marketcap_allocated?
    marketcap_allocated == true
  end

  def allocation0
    return super unless marketcap_allocated?

    result0 = base0_asset.get_market_cap
    result1 = base1_asset.get_market_cap
    if result0.success? && result1.success?
      (result0.data.to_f / (result0.data + result1.data)).round(2)
    else
      Rails.logger.error("Failed to get market cap for #{base0_asset.symbol}") if result0.failure?
      Rails.logger.error("Failed to get market cap for #{base1_asset.symbol}") if result1.failure?
      raise StandardError, "Failed to get market cap adjusted allocation for barbell bot #{id}"
    end
  end

  private

  def initialize_marketcap_allocatable_settings
    self.marketcap_allocated ||= false
  end
end
