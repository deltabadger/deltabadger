class Exchange < ApplicationRecord
  include ExchangeApi::BinanceEnum

  STABLE_TYPES = %w[Exchanges::Binance Exchanges::BinanceUs Exchanges::Coinbase Exchanges::Kraken].freeze

  has_many :bots
  has_many :api_keys
  has_one :fee_api_key
  has_many :exchange_assets
  has_many :assets, through: :exchange_assets
  has_many :tickers
  has_many :transactions

  validates :name, presence: true
  validates :type, uniqueness: true

  scope :available, -> { where(available: true) }

  def beta?
    !type.in?(STABLE_TYPES)
  end

  include Synchronizer
  include CandleBuilder

  def symbols
    market = case name.downcase
             when 'binance' then ExchangeApi::Markets::Binance::Market.new(url_base: EU_URL_BASE)
             when 'binance.us' then ExchangeApi::Markets::Binance::Market.new(url_base: US_URL_BASE)
             when 'kraken' then ExchangeApi::Markets::Kraken::Market.new
             when 'coinbase' then ExchangeApi::Markets::Coinbase::Market.new
             when 'bitget' then ExchangeApi::Markets::Bitget::Market.new
             when 'kucoin' then ExchangeApi::Markets::Kucoin::Market.new
             when 'bybit' then ExchangeApi::Markets::Bybit::Market.new
             when 'mexc' then ExchangeApi::Markets::Mexc::Market.new
             when 'gemini' then ExchangeApi::Markets::Gemini::Market.new
             when 'bitvavo' then ExchangeApi::Markets::Bitvavo::Market.new
             when 'hyperliquid' then ExchangeApi::Markets::Hyperliquid::Market.new
             when 'bingx' then ExchangeApi::Markets::Bingx::Market.new
             when 'bitrue' then ExchangeApi::Markets::Bitrue::Market.new
             when 'bitmart' then ExchangeApi::Markets::Bitmart::Market.new
             else
               Result::Failure.new("Unsupported exchange #{name}")
             end
    cache_key = "#{name.downcase}_all_symbols"
    market.all_symbols(cache_key)
  end

  def name_id
    self.class.name.demodulize.underscore
  end

  def coingecko_id
    raise NotImplementedError, "#{self.class.name} must implement coingecko_id"
  end

  def known_errors
    raise NotImplementedError, "#{self.class.name} must implement known_errors"
  end

  def set_client(api_key: nil)
    raise NotImplementedError, "#{self.class.name} must implement set_client"
  end

  def get_tickers_info(force: false)
    raise NotImplementedError, "#{self.class.name} must implement get_tickers_info"
  end

  def get_tickers_prices(force: false)
    raise NotImplementedError, "#{self.class.name} must implement get_tickers_prices"
  end

  def get_balances(asset_ids: nil)
    raise NotImplementedError, "#{self.class.name} must implement get_balances"
  end

  def get_balance(asset_id:)
    result = get_balances(asset_ids: [asset_id])
    return result if result.failure?

    Result::Success.new(result.data[asset_id])
  end

  def get_last_price(ticker:, force: false)
    raise NotImplementedError, "#{self.class.name} must implement get_last_price"
  end

  def get_bid_price(ticker:, force: false)
    raise NotImplementedError, "#{self.class.name} must implement get_bid_price"
  end

  def get_ask_price(ticker:, force: false)
    raise NotImplementedError, "#{self.class.name} must implement get_ask_price"
  end

  def get_candles(ticker:, start_at:, timeframe:)
    raise NotImplementedError, "#{self.class.name} must implement get_candles"
  end

  def market_buy(ticker:, amount:, amount_type:)
    raise NotImplementedError, "#{self.class.name} must implement market_buy"
  end

  def market_sell(ticker:, amount:, amount_type:)
    raise NotImplementedError, "#{self.class.name} must implement market_sell"
  end

  def limit_buy(ticker:, amount:, amount_type:, price:)
    raise NotImplementedError, "#{self.class.name} must implement limit_buy"
  end

  def limit_sell(ticker:, amount:, amount_type:, price:)
    raise NotImplementedError, "#{self.class.name} must implement limit_sell"
  end

  def get_order(order_id:)
    raise NotImplementedError, "#{self.class.name} must implement get_order"
  end

  def get_orders(order_ids:)
    raise NotImplementedError, "#{self.class.name} must implement get_orders"
  end

  def cancel_order(order_id:)
    raise NotImplementedError, "#{self.class.name} must implement cancel_order"
  end

  def supports_withdrawal?
    true
  end

  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
    raise NotImplementedError, "#{self.class.name} must implement withdraw"
  end

  def get_api_key_validity(api_key:)
    raise NotImplementedError, "#{self.class.name} must implement get_api_key_validity"
  end

  def fetch_withdrawal_fees!
    raise NotImplementedError, "#{self.class.name} must implement fetch_withdrawal_fees!"
  end

  def withdrawal_fee_for(asset:)
    ea = exchange_assets.find_by(asset: asset)
    return nil if ea.nil? || ea.withdrawal_fee.blank?

    BigDecimal(ea.withdrawal_fee)
  end

  def withdrawal_fee_fresh?(asset:)
    ea = exchange_assets.find_by(asset: asset)
    return false if ea.nil? || ea.withdrawal_fee_updated_at.nil?

    ea.withdrawal_fee_updated_at > 24.hours.ago
  end

  def minimum_amount_logic
    raise NotImplementedError, "#{self.class.name} must implement minimum_amount_logic"
  end

  def symbol_from_asset(asset)
    @symbol_from_asset ||= tickers.available.includes(:base_asset, :quote_asset).each_with_object({}) do |t, h|
      h[t.base_asset_id] ||= t.base
      h[t.quote_asset_id] ||= t.quote
    end
    @symbol_from_asset[asset.id]
  end

  def requires_passphrase?
    false
  end

  private

  def update_exchange_asset_fees!(fees, chains: {})
    updated = {}
    fees.each do |symbol, fee_string|
      asset = asset_from_symbol(symbol)
      next unless asset

      ea = exchange_assets.find_or_create_by!(asset: asset)
      attrs = { withdrawal_fee: fee_string, withdrawal_fee_updated_at: Time.current }
      attrs[:withdrawal_chains] = chains[symbol] if chains.key?(symbol)
      ea.update!(attrs)
      updated[symbol] = fee_string
    end
    Result::Success.new(updated)
  end
end
