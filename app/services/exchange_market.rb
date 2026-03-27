# Thin adapter wrapping Honeymaker::Exchange for use in the app.
# Replaces the old ExchangeApi::Markets subsystem.
class ExchangeMarket
  DEFAULT_FEE = 0.1

  def self.for(exchange_id)
    exchange = Exchange.find(exchange_id)
    return Fake.new if Rails.configuration.dry_run

    new(exchange)
  end

  def initialize(exchange)
    @exchange = exchange
    @honeymaker = Honeymaker.exchange(exchange.name_id)
  end

  def symbol(base, quote)
    result = @honeymaker.tickers_info
    return nil if result.failure?

    ticker = result.data.find { |t| t[:base] == base && t[:quote] == quote }
    ticker&.dig(:ticker)
  end

  def current_price(symbol)
    result = @honeymaker.get_price(symbol)
    wrap(result)
  end

  def current_bid_price(symbol)
    result = @honeymaker.get_bid_ask(symbol)
    return wrap_failure(result) if result.failure?

    Result::Success.new(result.data[:bid])
  end

  def current_ask_price(symbol)
    result = @honeymaker.get_bid_ask(symbol)
    return wrap_failure(result) if result.failure?

    Result::Success.new(result.data[:ask])
  end

  def base_decimals(symbol)
    ticker = @honeymaker.find_ticker(symbol)
    return wrap_failure(ticker) if ticker.failure?

    Result::Success.new(ticker.data[:base_decimals])
  end

  def quote_decimals(symbol)
    ticker = @honeymaker.find_ticker(symbol)
    return wrap_failure(ticker) if ticker.failure?

    Result::Success.new(ticker.data[:quote_decimals])
  end

  def minimum_order_parameters(symbol)
    ticker = @honeymaker.find_ticker(symbol)
    return wrap_failure(ticker) if ticker.failure?

    t = ticker.data
    Result::Success.new(
      minimum: BigDecimal(t[:minimum_quote_size] || '0'),
      minimum_quote: BigDecimal(t[:minimum_quote_size] || '0'),
      minimum_limit: BigDecimal(t[:minimum_base_size] || '0'),
      side: 'quote'
    )
  end

  def all_symbols(cache_key, expires_in = 1.hour)
    cached = Rails.cache.read(cache_key)
    return Result::Success.new(cached) if cached

    result = @honeymaker.symbols
    return wrap_failure(result) if result.failure?

    Rails.cache.write(cache_key, result.data, expires_in: expires_in)
    Result::Success.new(result.data)
  end

  def current_fee
    DEFAULT_FEE
  end

  def subaccounts(_api_keys)
    Result::Success.new([])
  end

  private

  def wrap(honeymaker_result)
    if honeymaker_result.success?
      Result::Success.new(honeymaker_result.data)
    else
      Result::Failure.new(*honeymaker_result.errors)
    end
  end

  def wrap_failure(honeymaker_result)
    Result::Failure.new(*honeymaker_result.errors)
  end

  # Fake market for dry_run mode
  class Fake
    DEFAULT_FEE = 0.1

    def symbol(_base, _quote)
      'FAKESYMBOL'
    end

    def current_price(_symbol)
      Result::Success.new(BigDecimal(rand(6000..8000).to_s))
    end

    def current_bid_price(_symbol)
      Result::Success.new(BigDecimal(rand(6000..8000).to_s))
    end

    def current_ask_price(_symbol)
      Result::Success.new(BigDecimal(rand(6000..8000).to_s))
    end

    def base_decimals(_symbol)
      Result::Success.new(8)
    end

    def quote_decimals(_symbol)
      Result::Success.new(8)
    end

    def minimum_order_parameters(_symbol)
      Result::Success.new(
        minimum: BigDecimal('0.001'),
        minimum_quote: BigDecimal('5'),
        minimum_limit: BigDecimal('0.001'),
        side: 'base'
      )
    end

    def all_symbols(_cache_key, _expires_in = nil)
      Result::Success.new([])
    end

    def current_fee
      DEFAULT_FEE
    end

    def subaccounts(_api_keys)
      Result::Success.new([])
    end
  end
end
