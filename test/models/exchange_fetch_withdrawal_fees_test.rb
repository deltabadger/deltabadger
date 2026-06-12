require 'test_helper'

# Characterization tests for the fetch_withdrawal_fees! scaffold shared by the
# authenticated-client exchanges (Binance, Binance.US, BingX, Bitrue, Bybit):
# fee_api_key blank-guard, Honeymaker.client construction (name string, key,
# secret, PROXY_* env var), failure passthrough, parse + fee update.
# The other implementations (public-endpoint clients, Kraken's memoized client,
# the empty stubs) are covered in exchange_withdrawal_fees_test.rb or untouched.
class ExchangeFetchWithdrawalFeesTest < ActiveSupport::TestCase
  setup do
    Rails.configuration.stubs(:dry_run).returns(false)
  end

  def build_exchange(factory)
    exchange = create(factory)
    asset = create(:asset)
    create(:ticker, exchange: exchange, base_asset: asset, quote_asset: create(:asset))
    [exchange, asset, ExchangeAsset.find_by(exchange: exchange, asset: asset)]
  end

  def with_env(key, value)
    old = ENV[key]
    ENV[key] = value
    yield
  ensure
    ENV[key] = old
  end

  def stub_fee_client(name, proxy_env, endpoint, result)
    hm_client = mock('honeymaker_client')
    hm_client.stubs(endpoint).returns(result)
    Honeymaker.expects(:client)
              .with(name, api_key: 'fee_key', api_secret: 'fee_secret', proxy: ENV[proxy_env])
              .returns(hm_client)
  end

  def binance_style_payload(symbol)
    [
      {
        'coin' => symbol,
        'networkList' => [
          { 'network' => 'BSC', 'isDefault' => false, 'withdrawFee' => '0.00001' },
          { 'network' => 'BTC', 'isDefault' => true, 'withdrawFee' => '0.0005' }
        ]
      }
    ]
  end

  # --- guard: blank fee_api_key returns empty success, no client built ---

  %i[binance_exchange binance_us_exchange bingx_exchange bitrue_exchange bybit_exchange].each do |factory|
    test "#{factory} fetch_withdrawal_fees! returns empty success without fee_api_key" do
      exchange = create(factory)
      Honeymaker.expects(:client).never

      result = exchange.fetch_withdrawal_fees!

      assert result.success?
      assert_equal({}, result.data)
    end
  end

  # --- Binance ---

  test 'Binance builds authenticated proxied client and picks the default network' do
    exchange, asset, ea = build_exchange(:binance_exchange)
    FeeApiKey.create!(exchange: exchange, key: 'fee_key', secret: 'fee_secret')

    with_env('PROXY_BINANCE', 'http://uk-proxy.test:8100') do
      stub_fee_client('binance', 'PROXY_BINANCE', :get_all_coins_information,
                      Result::Success.new(binance_style_payload(asset.symbol)))

      result = exchange.fetch_withdrawal_fees!

      assert result.success?
      ea.reload
      assert_equal '0.0005', ea.withdrawal_fee
      chains = ea.withdrawal_chains
      assert_equal 2, chains.size
      assert_equal ['BSC', false], [chains[0]['name'], chains[0]['is_default']]
      assert_equal ['BTC', true], [chains[1]['name'], chains[1]['is_default']]
    end
  end

  test 'Binance fetch_withdrawal_fees! passes a client failure through unchanged' do
    exchange, _asset, ea = build_exchange(:binance_exchange)
    FeeApiKey.create!(exchange: exchange, key: 'fee_key', secret: 'fee_secret')
    failure = Result::Failure.new('boom')

    with_env('PROXY_BINANCE', 'http://uk-proxy.test:8100') do
      stub_fee_client('binance', 'PROXY_BINANCE', :get_all_coins_information, failure)

      result = exchange.fetch_withdrawal_fees!

      assert result.failure?
      assert_same failure, result
      assert_nil ea.reload.withdrawal_fee
    end
  end

  # --- Binance.US (no chain data persisted) ---

  test 'BinanceUs builds authenticated proxied client and updates fees without chains' do
    exchange, asset, ea = build_exchange(:binance_us_exchange)
    FeeApiKey.create!(exchange: exchange, key: 'fee_key', secret: 'fee_secret')
    ea.update!(withdrawal_chains: [{ 'name' => 'OLD', 'fee' => '1', 'is_default' => true }])

    with_env('PROXY_BINANCE_US', 'http://uk-proxy.test:8100') do
      stub_fee_client('binance_us', 'PROXY_BINANCE_US', :get_all_coins_information,
                      Result::Success.new(binance_style_payload(asset.symbol)))

      result = exchange.fetch_withdrawal_fees!

      assert result.success?
      ea.reload
      assert_equal '0.0005', ea.withdrawal_fee
      # BinanceUs passes no chains: existing chain data must stay untouched
      assert_equal 'OLD', ea.withdrawal_chains.first['name']
    end
  end

  # --- BingX (payload nested under 'data') ---

  test 'BingX builds authenticated proxied client and parses the data envelope' do
    exchange, asset, ea = build_exchange(:bingx_exchange)
    FeeApiKey.create!(exchange: exchange, key: 'fee_key', secret: 'fee_secret')
    payload = { 'data' => binance_style_payload(asset.symbol) }

    with_env('PROXY_BINGX', 'http://uk-proxy.test:8100') do
      stub_fee_client('bingx', 'PROXY_BINGX', :get_all_coins_info,
                      Result::Success.new(payload))

      result = exchange.fetch_withdrawal_fees!

      assert result.success?
      ea.reload
      assert_equal '0.0005', ea.withdrawal_fee
      assert_equal 2, ea.withdrawal_chains.size
    end
  end

  # --- Bitrue (bare array payload) ---

  test 'Bitrue builds authenticated proxied client and parses the array payload' do
    exchange, asset, ea = build_exchange(:bitrue_exchange)
    FeeApiKey.create!(exchange: exchange, key: 'fee_key', secret: 'fee_secret')

    with_env('PROXY_BITRUE', 'http://uk-proxy.test:8100') do
      stub_fee_client('bitrue', 'PROXY_BITRUE', :get_all_coins_information,
                      Result::Success.new(binance_style_payload(asset.symbol)))

      result = exchange.fetch_withdrawal_fees!

      assert result.success?
      ea.reload
      assert_equal '0.0005', ea.withdrawal_fee
      assert_equal 2, ea.withdrawal_chains.size
    end
  end

  # --- Bybit (result/rows envelope, chainDefault string flags) ---

  test 'Bybit builds authenticated proxied client and picks the default chain' do
    exchange, asset, ea = build_exchange(:bybit_exchange)
    FeeApiKey.create!(exchange: exchange, key: 'fee_key', secret: 'fee_secret')
    payload = {
      'result' => {
        'rows' => [
          {
            'coin' => asset.symbol,
            'chains' => [
              { 'chain' => 'ARBI', 'chainDefault' => '0', 'withdrawFee' => '0.0001' },
              { 'chain' => 'ETH', 'chainDefault' => '1', 'withdrawFee' => '0.002' }
            ]
          }
        ]
      }
    }

    with_env('PROXY_BYBIT', 'http://uk-proxy.test:8100') do
      stub_fee_client('bybit', 'PROXY_BYBIT', :get_coin_query_info,
                      Result::Success.new(payload))

      result = exchange.fetch_withdrawal_fees!

      assert result.success?
      ea.reload
      assert_equal '0.002', ea.withdrawal_fee
      chains = ea.withdrawal_chains
      assert_equal 2, chains.size
      assert_equal ['ARBI', false], [chains[0]['name'], chains[0]['is_default']]
      assert_equal ['ETH', true], [chains[1]['name'], chains[1]['is_default']]
    end
  end
end
