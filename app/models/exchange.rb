class Exchange < ApplicationRecord
  include ExchangeApi::BinanceEnum
  include ExchangeApi::FtxEnum

  validates :name, presence: true

  scope :available, -> { where.not(name: ['FTX', 'FTX.US', 'Coinbase Pro']) }
  scope :available_for_barbell_bots, -> { where(name: ['Coinbase']) } # FIXME: Temporary until all exchanges are supported

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

  def set_client(api_key: nil)
    exchange_implementation.set_client(api_key: api_key)
  end

  # @returns
  #   #=> {
  #         symbol: [String],
  #         base_asset: [String],
  #         quote_asset: [String],
  #         minimum_base_size: [Float],
  #         minimum_quote_size: [Float],
  #         maximum_base_size: [Float],
  #         maximum_quote_size: [Float],
  #         base_decimals: [Integer],
  #         quote_decimals: [Integer],
  #         price_decimals: [Integer]
  #       }
  def get_symbol_info(base_asset:, quote_asset:)
    exchange_implementation.get_symbol_info(base_asset: base_asset, quote_asset: quote_asset)
  end

  def get_info
    exchange_implementation.get_info
  end

  def get_balance(asset: nil)
    exchange_implementation.get_balance(asset: asset)
  end

  def get_bid_price(base_asset:, quote_asset:)
    exchange_implementation.get_bid_price(base_asset: base_asset, quote_asset: quote_asset)
  end

  def get_ask_price(base_asset:, quote_asset:)
    exchange_implementation.get_ask_price(base_asset: base_asset, quote_asset: quote_asset)
  end

  # @param amount_type [Symbol] :base or :quote
  def market_buy(base_asset:, quote_asset:, amount:, amount_type:)
    exchange_implementation.market_buy(base_asset: base_asset, quote_asset: quote_asset, amount: amount, amount_type: amount_type)
  end

  # @param amount_type [Symbol] :base or :quote
  def market_sell(base_asset:, quote_asset:, amount:, amount_type:)
    exchange_implementation.market_sell(base_asset: base_asset, quote_asset: quote_asset, amount: amount,
                                        amount_type: amount_type)
  end

  # @param amount_type [Symbol] :base or :quote
  def limit_buy(base_asset:, quote_asset:, amount:, amount_type:, price:)
    exchange_implementation.limit_buy(base_asset: base_asset, quote_asset: quote_asset, amount: amount, amount_type: amount_type,
                                      price: price)
  end

  # @param amount_type [Symbol] :base or :quote
  def limit_sell(base_asset:, quote_asset:, amount:, amount_type:, price:)
    exchange_implementation.limit_sell(base_asset: base_asset, quote_asset: quote_asset, amount: amount,
                                       amount_type: amount_type, price: price)
  end

  def get_order(order_id:)
    exchange_implementation.get_order(order_id: order_id)
  end

  def check_valid_api_key?(api_key:)
    exchange_implementation.check_valid_api_key?(api_key: api_key)
  end

  # @param amount_type [Symbol] :base or :quote
  def get_adjusted_amount(base_asset:, quote_asset:, amount:, amount_type:, method: :floor)
    raise "Unsupported amount type #{amount_type}" unless %i[quote base].include?(amount_type)

    result = get_symbol_info(base_asset: base_asset, quote_asset: quote_asset)
    return result unless result.success?

    decimals = amount_type == :quote ? result.data[:quote_decimals] : result.data[:base_decimals]
    Result::Success.new(amount.send(method, decimals))
  end

  def get_adjusted_price(base_asset:, quote_asset:, price:, method: :floor)
    result = get_symbol_info(base_asset: base_asset, quote_asset: quote_asset)
    return result unless result.success?

    Result::Success.new(price.send(method, result.data[:price_decimals]))
  end

  private

  def exchange_implementation
    @exchange_implementation ||= case name.downcase
                                 #  when 'binance' then Exchanges::BinanceExchange.new(self)
                                 when 'coinbase' then Exchanges::CoinbaseExchange.new(self)
                                 else
                                   puts "Unsupported exchange #{name}"
                                   # raise NotImplementedError, "Unsupported exchange #{name}"
                                 end
  end

  def filter_free_plan_symbols(symbols)
    return symbols # disable free plan symbols limitation

    is_kraken = name.downcase == 'kraken'
    btc_eth = is_kraken ? %w[XBT ETH LTC XMR] : %w[BTC ETH LTC XMR]
    symbols.select { |s| btc_eth.include?(s.base) }
  end
end
