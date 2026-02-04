# Helper methods for mocking exchange API calls in tests
#
# Since DRY_RUN=true in the test environment, most exchange API calls are already
# mocked via the Exchange::Dryable concern. These helpers provide additional
# control for testing specific scenarios.
module ExchangeMockHelpers
  # Stub the ask price for a ticker
  def stub_ticker_ask_price(ticker, price:)
    ticker.stubs(:get_ask_price).returns(Result::Success.new(price))
  end

  # Stub the bid price for a ticker
  def stub_ticker_bid_price(ticker, price:)
    ticker.stubs(:get_bid_price).returns(Result::Success.new(price))
  end

  # Stub the last price for a ticker
  def stub_ticker_last_price(ticker, price:)
    ticker.stubs(:get_last_price).returns(Result::Success.new(price))
  end

  # Stub ticker price failure
  def stub_ticker_price_failure(ticker, error: "Price fetch failed")
    ticker.stubs(:get_ask_price).returns(Result::Failure.new(error))
    ticker.stubs(:get_bid_price).returns(Result::Failure.new(error))
    ticker.stubs(:get_last_price).returns(Result::Failure.new(error))
  end

  # Stub exchange balances
  def stub_exchange_balances(exchange, balances)
    exchange.stubs(:get_balances).returns(Result::Success.new(balances))
  end

  # Stub exchange balance for a single asset
  def stub_exchange_balance(exchange, asset_id:, free: 0, locked: 0)
    exchange.stubs(:get_balance).with(asset_id: asset_id)
      .returns(Result::Success.new({free: free, locked: locked}))
  end

  # Stub a successful market buy order
  def stub_market_buy_success(exchange, order_id: nil)
    order_id ||= "test-order-#{SecureRandom.hex(8)}"
    exchange.stubs(:market_buy).returns(Result::Success.new(order_id: order_id))
  end

  # Stub a failed market buy order
  def stub_market_buy_failure(exchange, error: "Insufficient funds")
    exchange.stubs(:market_buy).returns(Result::Failure.new(error))
  end

  # Stub a successful limit buy order
  def stub_limit_buy_success(exchange, order_id: nil)
    order_id ||= "test-order-#{SecureRandom.hex(8)}"
    exchange.stubs(:limit_buy).returns(Result::Success.new(order_id: order_id))
  end

  # Stub a failed limit buy order
  def stub_limit_buy_failure(exchange, error: "Order rejected")
    exchange.stubs(:limit_buy).returns(Result::Failure.new(error))
  end

  # Stub get_order response
  def stub_get_order(exchange, order_data)
    exchange.stubs(:get_order).returns(Result::Success.new(order_data))
  end

  # Convenience method to set up all common mocks for bot execution
  def setup_bot_execution_mocks(bot, price: 50000, order_id: nil)
    order_id ||= "test-order-#{SecureRandom.hex(8)}"

    # Handle both single asset (ticker) and dual asset (ticker0, ticker1) bots
    tickers = if bot.respond_to?(:ticker) && bot.ticker.present?
                [bot.ticker]
              elsif bot.respond_to?(:ticker0) && bot.respond_to?(:ticker1)
                [bot.ticker0, bot.ticker1].compact
              else
                []
              end

    # Stub price fetching for all tickers
    tickers.each do |ticker|
      stub_ticker_ask_price(ticker, price: price)
      stub_ticker_bid_price(ticker, price: price)
      stub_ticker_last_price(ticker, price: price)
    end

    # Stub market buy (used by default for DCA)
    stub_market_buy_success(bot.exchange, order_id: order_id)

    # Stub balance checks - handle both single and dual asset bots
    balances = {bot.quote_asset_id => {free: 10000, locked: 0}}
    if bot.respond_to?(:base_asset_id)
      balances[bot.base_asset_id] = {free: 1.0, locked: 0}
    end
    if bot.respond_to?(:base0_asset_id)
      balances[bot.base0_asset_id] = {free: 1.0, locked: 0}
    end
    if bot.respond_to?(:base1_asset_id)
      balances[bot.base1_asset_id] = {free: 1.0, locked: 0}
    end
    stub_exchange_balances(bot.exchange, balances)

    order_id
  end

  # Stub API key validation
  def stub_api_key_validation(exchange, valid: true)
    exchange.stubs(:get_api_key_validity).returns(Result::Success.new(valid))
  end
end
