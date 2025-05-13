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

  after_initialize :include_exchange_implementation

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

  def include_exchange_implementation
    case name.downcase
    when 'coinbase' then singleton_class.include(Exchanges::Coinbase)
    when 'kraken' then singleton_class.include(Exchanges::Kraken)
    end
  end

  # @param amount_type [Symbol] :base or :quote
  def adjusted_amount(base_asset_id:, quote_asset_id:, amount:, amount_type:, method: :floor)
    raise "Unsupported amount type #{amount_type}" unless %i[quote base].include?(amount_type)

    ticker = exchange_tickers.find_by(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
    return amount unless ticker.present?

    decimals = amount_type == :quote ? ticker.quote_decimals : ticker.base_decimals
    amount.send(method, decimals)
  end

  def adjusted_price(base_asset_id:, quote_asset_id:, price:, method: :floor)
    ticker = exchange_tickers.find_by(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
    return price unless ticker.present?

    decimals = ticker.price_decimals
    price.send(method, decimals)
  end

  private

  def filter_free_plan_symbols(symbols)
    return symbols # disable free plan symbols limitation

    is_kraken = name.downcase == 'kraken'
    btc_eth = is_kraken ? %w[XBT ETH LTC XMR] : %w[BTC ETH LTC XMR]
    symbols.select { |s| btc_eth.include?(s.base) }
  end
end
