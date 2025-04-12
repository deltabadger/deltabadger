class Exchange < ApplicationRecord
  include ExchangeApi::BinanceEnum
  include ExchangeApi::FtxEnum

  has_many :bots
  has_many :api_keys
  has_many :exchange_assets
  has_many :assets, through: :exchange_assets
  has_many :exchange_tickers
  has_many :tickers, class_name: 'ExchangeTicker' # alias for exchange_tickers
  has_many :transactions

  validates :name, presence: true

  scope :available, -> { where.not(name: ['FTX', 'FTX.US', 'Coinbase Pro']) }
  scope :available_for_barbell_bots, lambda {
                                       where(name: %w[Coinbase Kraken])
                                     } # FIXME: Temporary until all exchanges are supported

  include RemoteDataAggregator

  # rubocop:disable Metrics/CyclomaticComplexity
  def symbols
    market = case name.downcase
             when 'binance' then ExchangeApi::Markets::Binance::Market.new(url_base: EU_URL_BASE)
             when 'binance.us' then ExchangeApi::Markets::Binance::Market.new(url_base: US_URL_BASE)
             when 'zonda' then ExchangeApi::Markets::Zonda::Market.new
             when 'kraken' then ExchangeApi::Markets::Kraken::Market.new
             when 'coinbase pro' then ExchangeApi::Markets::CoinbasePro::Market.new
             when 'coinbase' then ExchangeApi::Markets::Coinbase::Market.new
             when 'gemini' then ExchangeApi::Markets::Gemini::Market.new
             when 'ftx' then ExchangeApi::Markets::Ftx::Market.new(url_base: FTX_EU_URL_BASE)
             when 'ftx.us' then ExchangeApi::Markets::Ftx::Market.new(url_base: FTX_US_URL_BASE)
             when 'bitso' then ExchangeApi::Markets::Bitso::Market.new
             when 'kucoin' then ExchangeApi::Markets::Kucoin::Market.new
             when 'bitfinex' then ExchangeApi::Markets::Bitfinex::Market.new
             when 'bitstamp' then ExchangeApi::Markets::Bitstamp::Market.new
             when 'probit global' then ExchangeApi::Markets::Probit::Market.new
             when 'probit' then ExchangeApi::Markets::Probit::Market.new
             else
               Result::Failure.new("Unsupported exchange #{name}")
             end
    cache_key = "#{name.downcase}_all_symbols"
    market.all_symbols(cache_key)
  end
  # rubocop:enable Metrics/CyclomaticComplexity

  def free_plan_symbols
    all_symbols = symbols
    return all_symbols unless all_symbols.success?

    Result::Success.new(filter_free_plan_symbols(all_symbols.data))
  end

  def set_exchange_implementation(api_key: nil)
    @exchange_implementation = case name.downcase
                               #  when 'binance' then Exchanges::BinanceExchange.new(self, api_key)
                               when 'coinbase' then Exchanges::CoinbaseExchange.new(self, api_key)
                               when 'kraken' then Exchanges::KrakenExchange.new(self, api_key)
                               else
                                 puts "Unsupported exchange #{name}"
                                 # raise NotImplementedError, "Unsupported exchange #{name}"
                               end
  end

  def coingecko_id
    exchange_implementation.coingecko_id
  end

  # @returns
  #   #=> {
  #         ticker: [String],
  #         base: [String],
  #         quote: [String],
  #         minimum_base_size: [Float],
  #         minimum_quote_size: [Float],
  #         maximum_base_size: [Float],
  #         maximum_quote_size: [Float],
  #         base_decimals: [Integer],
  #         quote_decimals: [Integer],
  #         price_decimals: [Integer]
  #       }
  def get_tickers_info
    exchange_implementation.get_tickers_info
  end

  def get_balances(asset_ids: nil)
    exchange_implementation.get_balances(asset_ids: asset_ids)
  end

  def get_balance(asset_id:)
    exchange_implementation.get_balance(asset_id: asset_id)
  end

  def get_last_price(base_asset_id:, quote_asset_id:)
    exchange_implementation.get_last_price(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
  end

  def get_bid_price(base_asset_id:, quote_asset_id:)
    exchange_implementation.get_bid_price(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
  end

  def get_ask_price(base_asset_id:, quote_asset_id:)
    exchange_implementation.get_ask_price(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
  end

  # @param amount_type [Symbol] :base or :quote
  def market_buy(base_asset_id:, quote_asset_id:, amount:, amount_type:)
    exchange_implementation.market_buy(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id, amount: amount,
                                       amount_type: amount_type)
  end

  # @param amount_type [Symbol] :base or :quote
  def market_sell(base_asset_id:, quote_asset_id:, amount:, amount_type:)
    exchange_implementation.market_sell(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id, amount: amount,
                                        amount_type: amount_type)
  end

  # @param amount_type [Symbol] :base or :quote
  def limit_buy(base_asset_id:, quote_asset_id:, amount:, amount_type:, price:)
    exchange_implementation.limit_buy(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id, amount: amount,
                                      amount_type: amount_type, price: price)
  end

  # @param amount_type [Symbol] :base or :quote
  def limit_sell(base_asset_id:, quote_asset_id:, amount:, amount_type:, price:)
    exchange_implementation.limit_sell(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id, amount: amount,
                                       amount_type: amount_type, price: price)
  end

  def get_order(order_id:)
    exchange_implementation.get_order(order_id: order_id)
  end

  def check_valid_api_key?(api_key:)
    exchange_implementation.check_valid_api_key?(api_key: api_key)
  end

  # @param amount_type [Symbol] :base or :quote
  def adjusted_amount(base_asset_id:, quote_asset_id:, amount:, amount_type:, method: :floor)
    raise "Unsupported amount type #{amount_type}" unless %i[quote base].include?(amount_type)

    ticker = exchange_tickers.find_by(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
    decimals = amount_type == :quote ? ticker.quote_decimals : ticker.base_decimals
    amount.send(method, decimals)
  end

  def adjusted_price(base_asset_id:, quote_asset_id:, price:, method: :floor)
    ticker = exchange_tickers.find_by(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
    decimals = ticker.price_decimals
    price.send(method, decimals)
  end

  private

  def exchange_implementation
    @exchange_implementation ||= set_exchange_implementation
  end

  def filter_free_plan_symbols(symbols)
    return symbols # disable free plan symbols limitation

    is_kraken = name.downcase == 'kraken'
    btc_eth = is_kraken ? %w[XBT ETH LTC XMR] : %w[BTC ETH LTC XMR]
    symbols.select { |s| btc_eth.include?(s.base) }
  end
end
