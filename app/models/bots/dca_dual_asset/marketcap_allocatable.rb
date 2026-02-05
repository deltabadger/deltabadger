module Bots::DcaDualAsset::MarketcapAllocatable
  extend ActiveSupport::Concern

  included do
    store_accessor :settings,
                   :marketcap_allocated

    after_initialize :initialize_marketcap_allocatable_settings

    validates :marketcap_allocated, inclusion: { in: [true, false] }

    decorators = Module.new do
      def parse_params(params)
        super(params).merge(
          marketcap_allocated: params[:marketcap_allocated].presence&.in?(%w[1 true])
        ).compact
      end
    end

    prepend decorators
  end

  def marketcap_allocated?
    marketcap_allocated == true
  end

  def allocation0
    return super unless marketcap_allocated?

    @allocation0_memoized ||= begin
      # Calculate dynamic market cap using circulating_supply * current_price
      marketcap0 = calculate_dynamic_market_cap(base0_asset)
      marketcap1 = calculate_dynamic_market_cap(base1_asset)

      if marketcap0.present? && marketcap0.positive? && marketcap1.present? && marketcap1.positive?
        (marketcap0.to_f / (marketcap0 + marketcap1)).round(2)
      else
        # Fall back to stored allocation0 value (default 50/50 split)
        Rails.logger.warn("Market cap data not available for bot #{id}. Using stored allocation: #{super}")
        super
      end
    end
  end

  private

  def calculate_dynamic_market_cap(asset)
    # If circulating supply is available from fixtures, calculate market cap dynamically
    if asset.circulating_supply.present? && asset.circulating_supply.positive?
      # Get current price (from exchange or CoinGecko)
      price_result = get_current_price(asset)
      return (asset.circulating_supply * price_result.data).to_f if price_result.success?

      Rails.logger.warn("Failed to get price for #{asset.symbol}: #{price_result.errors.join(', ')}")

    end

    # Fall back to static market cap from database
    asset.market_cap&.to_f
  end

  def get_current_price(asset)
    # First, try to get price from the exchange if a USD or USDT ticker exists
    if exchange.present?
      %w[USDT USD USDC BUSD].each do |quote_symbol|
        ticker = exchange.tickers.available.find_by(
          base_asset_id: asset.id,
          quote: quote_symbol
        )
        if ticker.present?
          price_result = exchange.get_last_price(ticker: ticker)
          return price_result if price_result.success?
        end
      end
    end

    # If a market data provider is configured, try to get price from there
    if MarketData.configured?
      result = asset.get_price(currency: 'usd')
      return result if result.success?
    end

    # No price available
    Result::Failure.new("No price data available for #{asset.symbol}")
  end

  def initialize_marketcap_allocatable_settings
    self.marketcap_allocated ||= false
  end
end
