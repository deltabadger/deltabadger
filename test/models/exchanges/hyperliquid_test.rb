require 'test_helper'

class Exchanges::HyperliquidTest < ActiveSupport::TestCase
  VALID_WALLET = '0x1234567890abcdef1234567890abcdef12345678'.freeze
  VALID_AGENT_KEY = "0x#{'ab' * 32}".freeze

  setup do
    @exchange = create(:hyperliquid_exchange)
  end

  test 'coingecko_id returns hyperliquid_spot' do
    assert_equal 'hyperliquid_spot', @exchange.coingecko_id
  end

  test 'known_errors returns expected error messages' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
  end

  test 'minimum_amount_logic returns base' do
    assert_equal :base, @exchange.minimum_amount_logic(side: :buy, order_type: :limit_order)
    assert_equal :base, @exchange.minimum_amount_logic(side: :sell, order_type: :limit_order)
  end

  test 'set_client creates a Clients::Hyperliquid instance' do
    @exchange.set_client
    assert_kind_of Clients::Hyperliquid, @exchange.send(:client)
  end

  test 'set_client with api_key stores the api_key' do
    api_key = create(:api_key, exchange: @exchange,
                               raw_key: VALID_WALLET,
                               raw_secret: VALID_AGENT_KEY)
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'requires_passphrase? returns false' do
    assert_equal false, @exchange.requires_passphrase?
  end

  test 'get_tickers_info returns formatted ticker data' do
    @exchange.set_client
    client = @exchange.send(:client)

    spot_meta = {
      'tokens' => [
        { 'name' => 'PURR', 'index' => 1, 'szDecimals' => 0 },
        { 'name' => 'USDC', 'index' => 0, 'szDecimals' => 2 },
        { 'name' => 'HYPE', 'index' => 2, 'szDecimals' => 2 }
      ],
      'universe' => [
        { 'name' => 'PURR/USDC', 'tokens' => [1, 0], 'index' => 1000 },
        { 'name' => 'HYPE/USDC', 'tokens' => [2, 0], 'index' => 1001 }
      ]
    }
    client.stubs(:spot_meta).returns(Result::Success.new(spot_meta))

    result = @exchange.get_tickers_info(force: true)
    assert result.success?
    assert_equal 2, result.data.size

    purr_ticker = result.data.find { |t| t[:ticker] == 'PURR/USDC' }
    assert_equal 'PURR', purr_ticker[:base]
    assert_equal 'USDC', purr_ticker[:quote]
    assert_equal 0, purr_ticker[:base_decimals]
    assert purr_ticker[:available]
  end

  test 'get_tickers_prices returns price hash' do
    @exchange.set_client
    client = @exchange.send(:client)

    mids_data = { 'PURR/USDC' => '2.50', 'HYPE/USDC' => '25.00' }
    client.stubs(:all_mids).returns(Result::Success.new(mids_data))

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                    ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC')

    result = @exchange.get_tickers_prices(force: true)
    assert result.success?
    assert_equal '2.50'.to_d, result.data['PURR/USDC']
  end

  test 'get_last_price returns mid price' do
    @exchange.set_client
    client = @exchange.send(:client)

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    ticker = create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                             ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC')

    mids_data = { 'PURR/USDC' => '2.50' }
    client.stubs(:all_mids).returns(Result::Success.new(mids_data))

    result = @exchange.get_last_price(ticker: ticker, force: true)
    assert result.success?
    assert_equal '2.50'.to_d, result.data
  end

  test 'get_bid_price returns best bid from l2_book' do
    @exchange.set_client
    client = @exchange.send(:client)

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    ticker = create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                             ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC')

    book_data = {
      'levels' => [
        [{ 'px' => '2.49', 'sz' => '100', 'n' => 1 }],
        [{ 'px' => '2.51', 'sz' => '50', 'n' => 1 }]
      ]
    }
    client.stubs(:l2_book).returns(Result::Success.new(book_data))

    result = @exchange.get_bid_price(ticker: ticker, force: true)
    assert result.success?
    assert_equal '2.49'.to_d, result.data
  end

  test 'get_ask_price returns best ask from l2_book' do
    @exchange.set_client
    client = @exchange.send(:client)

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    ticker = create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                             ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC')

    book_data = {
      'levels' => [
        [{ 'px' => '2.49', 'sz' => '100', 'n' => 1 }],
        [{ 'px' => '2.51', 'sz' => '50', 'n' => 1 }]
      ]
    }
    client.stubs(:l2_book).returns(Result::Success.new(book_data))

    result = @exchange.get_ask_price(ticker: ticker, force: true)
    assert result.success?
    assert_equal '2.51'.to_d, result.data
  end

  test 'market_buy raises because Hyperliquid has no native market orders' do
    @exchange.set_client

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    ticker = create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                             ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC')

    error = assert_raises(RuntimeError) do
      @exchange.market_buy(ticker: ticker, amount: 10, amount_type: :base)
    end
    assert_match(/does not support market orders/, error.message)
  end

  test 'market_sell raises because Hyperliquid has no native market orders' do
    @exchange.set_client

    usdc = create(:asset, external_id: 'usdc', symbol: 'USDC', name: 'USDC')
    purr = create(:asset, external_id: 'purr', symbol: 'PURR', name: 'Purr')
    ticker = create(:ticker, exchange: @exchange, base_asset: purr, quote_asset: usdc,
                             ticker: 'PURR/USDC', base: 'PURR', quote: 'USDC')

    error = assert_raises(RuntimeError) do
      @exchange.market_sell(ticker: ticker, amount: 10, amount_type: :base)
    end
    assert_match(/does not support market orders/, error.message)
  end
end
