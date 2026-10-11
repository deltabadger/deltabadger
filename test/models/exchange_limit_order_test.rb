require 'test_helper'

# Exchange#limit_buy / #limit_sell hand every venue a BASE quantity: a quote amount is converted at
# the (adjusted) limit price and floored to the ticker's base decimals, a non-positive limit price
# is refused, and so is a quantity below the venue minimum. The REST/MCP limit_buy defaults to
# amount_type: quote, so without this a venue that takes base only got `quantity: nil` (Binance
# family, MEXC, Bitrue) or read the quote figure as a base amount (Gemini).
class ExchangeLimitOrderTest < ActiveSupport::TestCase
  PRICE = BigDecimal('62795.37')

  def ticker_for(exchange, **attrs)
    create(:ticker, exchange: exchange, base_symbol: 'BTC', quote_symbol: 'USDT',
                    price_decimals: 2, base_decimals: 6, minimum_base_size: 0.00001, **attrs)
  end

  # 100 USDT at the adjusted limit price, floored: 100 / 62795.37 = 0.0015924... -> 0.001592
  def expected_base(ticker)
    ticker.adjusted_amount(amount: BigDecimal('100') / ticker.adjusted_price(price: PRICE), amount_type: :base)
  end

  # Replaces one client method and records the keyword arguments it was called with.
  def capture(exchange, method, returns:)
    captured = {}
    exchange.send(:client).define_singleton_method(method) do |**kwargs|
      captured.merge!(kwargs)
      Result::Success.new(returns)
    end
    captured
  end

  def quote_limit_buy(exchange, ticker, price: PRICE, amount: BigDecimal('100'))
    with_dry_run(false) do
      exchange.limit_buy(ticker: ticker, amount: amount, amount_type: :quote, price: price)
    end
  end

  # == The venues that used to send no quantity for a quote amount ==

  { binance_exchange: 'Binance', binance_us_exchange: 'Binance.US', mexc_exchange: 'MEXC',
    bitrue_exchange: 'Bitrue' }.each do |factory, label|
    test "#{label} limit_buy with a quote amount sends a base quantity at the limit price" do
      exchange = create(factory)
      ticker = ticker_for(exchange)
      captured = capture(exchange, :new_order, returns: { order_id: "#{ticker.ticker}-1" })

      result = quote_limit_buy(exchange, ticker)

      assert_predicate result, :success?
      assert_equal '0.001592', captured[:quantity]
      assert_equal expected_base(ticker).to_s('F'), captured[:quantity]
      assert_equal 'LIMIT', captured[:type]
    end
  end

  # == The venues that took a quote amount some other way ==

  test 'Gemini limit_buy with a quote amount sends a base amount, not the quote figure' do
    exchange = create(:gemini_exchange)
    ticker = ticker_for(exchange)
    captured = capture(exchange, :new_order, returns: { order_id: 1 })

    assert_predicate quote_limit_buy(exchange, ticker), :success?
    assert_equal '0.001592', captured[:amount]
  end

  test 'Kraken limit_buy with a quote amount sends a base volume without viqc' do
    exchange = create(:kraken_exchange)
    ticker = ticker_for(exchange)
    captured = capture(exchange, :add_order, returns: { order_id: 'OABC-123' })

    assert_predicate quote_limit_buy(exchange, ticker), :success?
    assert_equal '0.001592', captured[:volume]
    assert_empty captured[:oflags]
  end

  test 'Coinbase limit_buy with a quote amount sends base_size' do
    exchange = create(:coinbase_exchange)
    ticker = ticker_for(exchange)
    captured = capture(exchange, :create_order, returns: { order_id: 'abc' })

    assert_predicate quote_limit_buy(exchange, ticker), :success?
    assert_equal({ base_size: '0.001592', limit_price: '62795.37' }, captured.dig(:order_configuration, :limit_limit_gtc))
  end

  test 'Coinbase bots size limit orders in base and keep the cheaper choice for market orders' do
    exchange = create(:coinbase_exchange)
    assert_equal :base_and_quote_in_base, exchange.minimum_amount_logic(side: :buy, order_type: :limit_order)
    assert_equal :base_or_quote, exchange.minimum_amount_logic(side: :buy, order_type: :market_order)
  end

  # Alpaca bots pre-check only the floored quote amount, so the base quantity is what has to be refused.
  [['0.0001', 4, '64160.69', 'Limit order of 0.0 BTC is below the minimum of 0.0001 BTC'],
   ['0.000027', 9, '50000', 'Limit order of 0.00002 BTC is below the minimum of 0.000027 BTC']].each do |min, decimals, price, message|
    test "Alpaca refuses a 1 USD limit buy at #{price} whose quantity is under #{min} BTC" do
      exchange = create(:alpaca_exchange)
      ticker = ticker_for(exchange, minimum_base_size: BigDecimal(min), base_decimals: decimals)
      exchange.send(:client).expects(:create_order).never

      result = quote_limit_buy(exchange, ticker, amount: BigDecimal('1'), price: BigDecimal(price))

      assert_predicate result, :failure?
      assert_includes result.errors.join, message
    end
  end

  # == Every venue goes through the same sizing ==

  LIVE_FACTORIES = %i[binance_exchange binance_us_exchange kraken_exchange coinbase_exchange bitget_exchange
                      kucoin_exchange bybit_exchange mexc_exchange gemini_exchange bitvavo_exchange
                      hyperliquid_exchange bingx_exchange bitrue_exchange alpaca_exchange ibkr_exchange].freeze

  LIVE_FACTORIES.each do |factory|
    test "#{factory} limit_buy and limit_sell hand the venue a floored base amount" do
      exchange = create(factory)
      ticker = ticker_for(exchange)
      calls = []
      exchange.define_singleton_method(:set_limit_order) do |**kwargs|
        calls << kwargs
        Result::Success.new(order_id: 'x')
      end

      quote_limit_buy(exchange, ticker)
      with_dry_run(false) do
        exchange.limit_sell(ticker: ticker, amount: BigDecimal('0.0015999'), amount_type: :base, price: PRICE)
      end

      sent = calls.map { |c| [c[:amount_type], c[:amount], c[:side]] }
      assert_equal [[:base, BigDecimal('0.001592'), :buy], [:base, BigDecimal('0.001599'), :sell]], sent
    end

    test "#{factory} refuses a limit order whose price is not positive" do
      exchange = create(factory)
      ticker = ticker_for(exchange)
      exchange.expects(:set_limit_order).never

      [BigDecimal('0'), BigDecimal('-1')].each do |price|
        result = quote_limit_buy(exchange, ticker, price: price)
        assert_predicate result, :failure?
        assert_includes result.errors.join, 'limit price'
      end
    end

    test "#{factory} refuses a limit order whose floored quantity is below the venue minimum" do
      exchange = create(factory)
      ticker = ticker_for(exchange)
      exchange.expects(:set_limit_order).never

      # 0.5 USDT buys 0.000007 BTC, under the 0.00001 floor; 0.00000001 BTC floors to zero at 6 decimals.
      below_quote = quote_limit_buy(exchange, ticker, amount: BigDecimal('0.5'))
      to_zero = with_dry_run(false) do
        exchange.limit_sell(ticker: ticker, amount: BigDecimal('0.00000001'), amount_type: :base, price: PRICE)
      end

      [below_quote, to_zero].each do |result|
        assert_predicate result, :failure?
        assert_includes result.errors.join, 'below'
      end
    end
  end
end
