require 'test_helper'

class Exchanges::BitmartTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:bitmart_exchange)
  end

  test 'coingecko_id returns bitmart' do
    assert_equal 'bitmart', @exchange.coingecko_id
  end

  test 'known_errors returns expected error messages' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_includes errors[:insufficient_funds], 'Balance not enough'
    assert_includes errors[:invalid_key], 'Invalid ACCESS_KEY'
  end

  test 'minimum_amount_logic returns base_and_quote' do
    assert_equal :base_and_quote, @exchange.minimum_amount_logic
  end

  test 'set_client creates a Honeymaker BitMart client' do
    @exchange.set_client
    assert_kind_of Honeymaker::Clients::BitMart, @exchange.send(:client)
  end

  test 'set_client with api_key stores the api_key' do
    api_key = create(:api_key, exchange: @exchange, raw_passphrase: 'test_memo')
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'requires_passphrase? returns true' do
    assert_equal true, @exchange.requires_passphrase?
  end

  test 'get_api_key_validity uses cancel_order for trading keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret',
                               raw_passphrase: 'test_memo')

    Honeymaker::Clients::BitMart.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'code' => 50_030, 'message' => 'Order not found' })
    )
    Honeymaker::Clients::BitMart.any_instance.expects(:get_wallet).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity uses get_wallet for withdrawal keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret',
                               raw_passphrase: 'test_memo')

    Honeymaker::Clients::BitMart.any_instance.stubs(:get_wallet).returns(
      Result::Success.new({ 'code' => 1000, 'data' => { 'wallet' => [] } })
    )
    Honeymaker::Clients::BitMart.any_instance.expects(:cancel_order).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns false for invalid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'bad_key', secret: 'bad_secret',
                               raw_passphrase: 'test_memo')

    Honeymaker::Clients::BitMart.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'code' => 30_006, 'message' => 'Invalid ACCESS_KEY' })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  # ---- get_candles deep-history pagination (the ATH ~20y lookback) ----
  #
  # BitMart caps a page well below the requested `limit` (~200), so the old
  # `break if items.size < limit` terminated after the first short page and left
  # the series stale (never reaching the present). Pagination must be bounded by
  # `before`/`after_time` and terminate on reaching the present, not on page size.

  test 'get_candles does not stop early on a short page and reaches the present' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    now = Time.now.utc
    listing = now - 600.days
    page_cap = 200

    client = Object.new
    client.define_singleton_method(:get_klines) do |**kw|
      lo = [kw[:after_time], listing.to_i].max
      hi = kw[:before] ? [kw[:before], now.to_i].min : now.to_i
      data = []
      t = lo
      while t <= hi && data.size < page_cap
        # BitMart candle order: [time_s, open, high, low, close, volume]
        data << [t.to_s, '10', '12', '9', '11', '100']
        t += 1.day.to_i
      end
      Result::Success.new({ 'data' => data })
    end
    @exchange.stubs(:client).returns(client)

    result = @exchange.get_candles(ticker: ticker, start_at: listing - 10.days, timeframe: 1.day)

    assert_predicate result, :success?
    assert_operator result.data.size, :>, page_cap, 'must paginate past a single short (< limit) page'
    assert_operator result.data.last[0], :>=, now - 2.days, 'pagination should reach the present'
  end

  test 'get_candles dedupes and globally sorts when windows overlap at the boundary' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'BTC', quote_symbol: 'USDT')
    now = Time.now.utc

    client = Object.new
    # Each window deliberately also returns the candle one day BEFORE its start, so adjacent
    # windows overlap by a candle (a misbehaving-API scenario the guard must absorb).
    client.define_singleton_method(:get_klines) do |**kw|
      lo = kw[:after_time] - 1.day.to_i
      hi = [kw[:before], now.to_i].min
      data = []
      t = lo
      while t <= hi
        data << [t.to_s, '10', '12', '9', '11', '100']
        t += 1.day.to_i
      end
      Result::Success.new({ 'data' => data })
    end
    @exchange.stubs(:client).returns(client)

    result = @exchange.get_candles(ticker: ticker, start_at: 800.days.ago, timeframe: 1.day)

    assert_predicate result, :success?
    timestamps = result.data.map { |c| c[0] }
    assert_equal timestamps.sort, timestamps, 'result must be globally sorted'
    assert_equal timestamps.uniq, timestamps, 'result must not contain duplicate candles'
  end
end
