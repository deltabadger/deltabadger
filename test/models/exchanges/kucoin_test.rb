require 'test_helper'

class Exchanges::KucoinTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:kucoin_exchange)
  end

  test 'coingecko_id returns kucoin' do
    assert_equal 'kucoin', @exchange.coingecko_id
  end

  test 'known_errors includes insufficient_funds and invalid_key' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_kind_of Array, errors[:insufficient_funds]
    assert_kind_of Array, errors[:invalid_key]
  end

  test 'minimum_amount_logic returns base_or_quote' do
    assert_equal :base_or_quote, @exchange.minimum_amount_logic
  end

  test 'requires_passphrase? returns true' do
    assert_predicate @exchange, :requires_passphrase?
  end

  test 'set_client creates Honeymaker::Clients::Kucoin instance' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: 'test_pass')
    @exchange.set_client(api_key: api_key)
    assert_kind_of Honeymaker::Clients::Kucoin, @exchange.instance_variable_get(:@client)
  end

  test 'set_client sets api_key reader' do
    api_key = stub(key: 'test_key', secret: 'test_secret', passphrase: 'test_pass')
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'set_client handles nil api_key' do
    @exchange.set_client(api_key: nil)
    assert_nil @exchange.api_key
    assert_kind_of Honeymaker::Clients::Kucoin, @exchange.instance_variable_get(:@client)
  end

  test 'get_api_key_validity uses cancel_order for trading keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret',
                               raw_passphrase: 'test_pass')

    Honeymaker::Clients::Kucoin.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'code' => '400100', 'msg' => 'Order does not exist' })
    )
    Honeymaker::Clients::Kucoin.any_instance.expects(:get_accounts).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity uses get_accounts for withdrawal keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret',
                               raw_passphrase: 'test_pass')

    Honeymaker::Clients::Kucoin.any_instance.stubs(:get_accounts).returns(
      Result::Success.new({ 'code' => '200000', 'data' => [] })
    )
    Honeymaker::Clients::Kucoin.any_instance.expects(:cancel_order).never

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity returns false for invalid trading key' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'bad_key', secret: 'bad_secret',
                               raw_passphrase: 'test_pass')

    Honeymaker::Clients::Kucoin.any_instance.stubs(:cancel_order).returns(
      Result::Success.new({ 'code' => '400003', 'msg' => 'Invalid KC-API-SIGN' })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  # ---- get_candles deep-history pagination (the ATH ~20y lookback) ----
  #
  # KuCoin requests a bounded [start_at, end_at] window. A 20-years-ago start_at
  # predates the listing, so the first window comes back empty. The loop must skip
  # forward through those empty windows and reach the present, not bail on the
  # first blank page (which left "% from ATH" bots frozen forever).

  test 'get_candles skips empty pre-listing windows and reaches the present' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'AVA', quote_symbol: 'USDT')
    now = Time.now.utc
    listing = now - 400.days

    client = Object.new
    client.define_singleton_method(:get_klines) do |**kw|
      lo = [kw[:start_at], listing.to_i].max
      hi = [kw[:end_at], now.to_i].min
      data = []
      t = lo
      while t <= hi
        # KuCoin candle order: [time, open, close, high, low, volume]
        data << [t.to_s, '10', '11', '12', '9', '100']
        t += 1.day.to_i
      end
      Result::Success.new({ 'data' => data })
    end
    @exchange.stubs(:client).returns(client)

    result = @exchange.get_candles(ticker: ticker, start_at: 2900.days.ago, timeframe: 1.day)

    assert_predicate result, :success?
    assert result.data.any?, 'expected candles after skipping the empty pre-listing windows'
    assert_operator result.data.last[0], :>=, now - 2.days, 'pagination should reach the present'
    assert_operator result.data.first[0], :>=, listing - 2.days, 'should not fabricate pre-listing candles'
  end

  test 'get_candles dedupes and globally sorts when windows overlap at the boundary' do
    ticker = create(:ticker, exchange: @exchange, base_symbol: 'AVA', quote_symbol: 'USDT')
    now = Time.now.utc

    client = Object.new
    # Each window deliberately also returns the candle one day BEFORE its start, so adjacent
    # windows overlap by a candle (a misbehaving-API scenario the guard must absorb).
    client.define_singleton_method(:get_klines) do |**kw|
      lo = kw[:start_at] - 1.day.to_i
      hi = [kw[:end_at], now.to_i].min
      data = []
      t = lo
      while t <= hi
        data << [t.to_s, '10', '11', '12', '9', '100']
        t += 1.day.to_i
      end
      Result::Success.new({ 'data' => data })
    end
    @exchange.stubs(:client).returns(client)

    result = @exchange.get_candles(ticker: ticker, start_at: 2000.days.ago, timeframe: 1.day)

    assert_predicate result, :success?
    timestamps = result.data.map { |c| c[0] }
    assert_equal timestamps.sort, timestamps, 'result must be globally sorted'
    assert_equal timestamps.uniq, timestamps, 'result must not contain duplicate candles'
  end
end
