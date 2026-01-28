# Helper methods for mocking exchange API calls in specs
#
# Since DRY_RUN=true in the test environment, most exchange API calls are already
# mocked via the Exchange::Dryable concern. These helpers provide additional
# control for testing specific scenarios.
module ExchangeMockHelpers
  # Stub the ask price for a ticker
  # @param ticker [Ticker] the ticker to stub
  # @param price [Numeric] the price to return
  # @return [Result::Success] with the price
  def stub_ticker_ask_price(ticker, price:)
    allow(ticker).to receive(:get_ask_price).and_return(Result::Success.new(price))
  end

  # Stub the bid price for a ticker
  # @param ticker [Ticker] the ticker to stub
  # @param price [Numeric] the price to return
  # @return [Result::Success] with the price
  def stub_ticker_bid_price(ticker, price:)
    allow(ticker).to receive(:get_bid_price).and_return(Result::Success.new(price))
  end

  # Stub the last price for a ticker
  # @param ticker [Ticker] the ticker to stub
  # @param price [Numeric] the price to return
  # @return [Result::Success] with the price
  def stub_ticker_last_price(ticker, price:)
    allow(ticker).to receive(:get_last_price).and_return(Result::Success.new(price))
  end

  # Stub ticker price failure
  # @param ticker [Ticker] the ticker to stub
  # @param error [String] the error message
  def stub_ticker_price_failure(ticker, error: 'Price fetch failed')
    allow(ticker).to receive(:get_ask_price).and_return(Result::Failure.new(error))
    allow(ticker).to receive(:get_bid_price).and_return(Result::Failure.new(error))
    allow(ticker).to receive(:get_last_price).and_return(Result::Failure.new(error))
  end

  # Stub exchange balances
  # @param exchange [Exchange] the exchange to stub
  # @param balances [Hash] asset_id => { free: amount, locked: amount }
  def stub_exchange_balances(exchange, balances)
    allow(exchange).to receive(:get_balances).and_return(Result::Success.new(balances))
  end

  # Stub exchange balance for a single asset
  # @param exchange [Exchange] the exchange to stub
  # @param asset_id [Integer] the asset ID
  # @param free [Numeric] free balance amount
  # @param locked [Numeric] locked balance amount
  def stub_exchange_balance(exchange, asset_id:, free: 0, locked: 0)
    allow(exchange).to receive(:get_balance)
      .with(asset_id: asset_id)
      .and_return(Result::Success.new({ free: free, locked: locked }))
  end

  # Stub a successful market buy order
  # @param exchange [Exchange] the exchange to stub
  # @param order_id [String] the order ID to return (defaults to a generated one)
  def stub_market_buy_success(exchange, order_id: nil)
    order_id ||= "test-order-#{SecureRandom.hex(8)}"
    allow(exchange).to receive(:market_buy).and_return(
      Result::Success.new(order_id: order_id)
    )
  end

  # Stub a failed market buy order
  # @param exchange [Exchange] the exchange to stub
  # @param error [String] the error message
  def stub_market_buy_failure(exchange, error: 'Insufficient funds')
    allow(exchange).to receive(:market_buy).and_return(
      Result::Failure.new(error)
    )
  end

  # Stub a successful limit buy order
  # @param exchange [Exchange] the exchange to stub
  # @param order_id [String] the order ID to return
  def stub_limit_buy_success(exchange, order_id: nil)
    order_id ||= "test-order-#{SecureRandom.hex(8)}"
    allow(exchange).to receive(:limit_buy).and_return(
      Result::Success.new(order_id: order_id)
    )
  end

  # Stub a failed limit buy order
  # @param exchange [Exchange] the exchange to stub
  # @param error [String] the error message
  def stub_limit_buy_failure(exchange, error: 'Order rejected')
    allow(exchange).to receive(:limit_buy).and_return(
      Result::Failure.new(error)
    )
  end

  # Stub get_order response
  # @param exchange [Exchange] the exchange to stub
  # @param order_data [Hash] the order data to return
  def stub_get_order(exchange, order_data)
    allow(exchange).to receive(:get_order).and_return(
      Result::Success.new(order_data)
    )
  end

  # Convenience method to set up all common mocks for bot execution
  # @param bot [Bot] the bot to set up mocks for
  # @param price [Numeric] the price to use for all price stubs (default: 50000)
  # @param order_id [String] the order ID to return (optional)
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
    balances = { bot.quote_asset_id => { free: 10000, locked: 0 } }
    if bot.respond_to?(:base_asset_id)
      balances[bot.base_asset_id] = { free: 1.0, locked: 0 }
    end
    if bot.respond_to?(:base0_asset_id)
      balances[bot.base0_asset_id] = { free: 1.0, locked: 0 }
    end
    if bot.respond_to?(:base1_asset_id)
      balances[bot.base1_asset_id] = { free: 1.0, locked: 0 }
    end
    stub_exchange_balances(bot.exchange, balances)

    order_id
  end

  # Stub API key validation
  # @param exchange [Exchange] the exchange to stub
  # @param valid [Boolean] whether the API key should be valid
  def stub_api_key_validation(exchange, valid: true)
    allow(exchange).to receive(:get_api_key_validity).and_return(
      Result::Success.new(valid)
    )
  end
end

RSpec.configure do |config|
  config.include ExchangeMockHelpers
end
