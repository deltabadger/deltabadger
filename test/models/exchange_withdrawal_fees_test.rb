require 'test_helper'

class ExchangeWithdrawalFeesTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    @asset = create(:asset)
    @quote_asset = create(:asset)
    # Ticker needed so asset_from_symbol can resolve the symbol
    create(:ticker, exchange: @exchange, base_asset: @asset, quote_asset: @quote_asset)
    @ea = ExchangeAsset.find_by(exchange: @exchange, asset: @asset)
  end

  # withdrawal_fee_for tests

  test 'withdrawal_fee_for returns BigDecimal when fee is cached' do
    @ea.update!(withdrawal_fee: '0.0005', withdrawal_fee_updated_at: Time.current)

    result = @exchange.withdrawal_fee_for(asset: @asset)
    assert_equal BigDecimal('0.0005'), result
  end

  test 'withdrawal_fee_for returns nil when no exchange_asset exists' do
    other_asset = create(:asset)

    assert_nil @exchange.withdrawal_fee_for(asset: other_asset)
  end

  test 'withdrawal_fee_for returns nil when withdrawal_fee is blank' do
    assert_nil @exchange.withdrawal_fee_for(asset: @asset)
  end

  # withdrawal_fee_fresh? tests

  test 'withdrawal_fee_fresh? returns true when updated within 24 hours' do
    @ea.update!(withdrawal_fee: '0.0005', withdrawal_fee_updated_at: 1.hour.ago)

    assert @exchange.withdrawal_fee_fresh?(asset: @asset)
  end

  test 'withdrawal_fee_fresh? returns false when updated more than 24 hours ago' do
    @ea.update!(withdrawal_fee: '0.0005', withdrawal_fee_updated_at: 25.hours.ago)

    assert_not @exchange.withdrawal_fee_fresh?(asset: @asset)
  end

  test 'withdrawal_fee_fresh? returns false when never updated' do
    assert_not @exchange.withdrawal_fee_fresh?(asset: @asset)
  end

  test 'withdrawal_fee_fresh? returns false for unknown asset' do
    other_asset = create(:asset)

    assert_not @exchange.withdrawal_fee_fresh?(asset: other_asset)
  end

  # fetch_withdrawal_fees! in dry_run mode (no fee_api_key)

  test 'fetch_withdrawal_fees! without fee_api_key returns empty success' do
    result = @exchange.fetch_withdrawal_fees!

    assert result.success?
    assert_equal({}, result.data)
  end

  # Binance fetch_withdrawal_fees! with mock API response

  test 'Binance fetch_withdrawal_fees! parses API response and updates exchange_assets' do
    Rails.configuration.stubs(:dry_run).returns(false)

    FeeApiKey.create!(exchange: @exchange, key: 'test_key', secret: 'test_secret')

    api_response = [
      {
        'coin' => @asset.symbol,
        'networkList' => [
          { 'network' => 'BTC', 'isDefault' => true, 'withdrawFee' => '0.0005' },
          { 'network' => 'BSC', 'isDefault' => false, 'withdrawFee' => '0.00001' }
        ]
      },
      {
        'coin' => 'UNKNOWN_COIN',
        'networkList' => [
          { 'network' => 'ETH', 'isDefault' => true, 'withdrawFee' => '0.01' }
        ]
      }
    ]

    Clients::Binance.any_instance
                    .stubs(:get_all_coins_information)
                    .returns(Result::Success.new(api_response))

    result = @exchange.fetch_withdrawal_fees!

    assert result.success?
    @ea.reload
    assert_equal '0.0005', @ea.withdrawal_fee
    assert_not_nil @ea.withdrawal_fee_updated_at

    # Verify chain data is persisted
    chains = @ea.withdrawal_chains
    assert_equal 2, chains.size
    assert_equal 'BTC', chains[0]['name']
    assert_equal '0.0005', chains[0]['fee']
    assert_equal true, chains[0]['is_default']
    assert_equal 'BSC', chains[1]['name']
    assert_equal false, chains[1]['is_default']
  end

  test 'Binance fetch_withdrawal_fees! returns empty success when no fee_api_key' do
    Rails.configuration.stubs(:dry_run).returns(false)

    result = @exchange.fetch_withdrawal_fees!

    assert result.success?
    assert_equal({}, result.data)
  end

  # Bitget fetch_withdrawal_fees! with mock API response

  test 'Bitget fetch_withdrawal_fees! parses API response and updates exchange_assets' do
    Rails.configuration.stubs(:dry_run).returns(false)

    exchange = create(:bitget_exchange)
    asset = create(:asset)
    create(:exchange_asset, exchange: exchange, asset: asset)
    # Create a ticker so asset_from_symbol can resolve the symbol
    create(:ticker, exchange: exchange, base_asset: asset, quote_asset: create(:asset))
    ea = ExchangeAsset.find_by(exchange: exchange, asset: asset)

    api_response = {
      'data' => [
        {
          'coin' => asset.symbol,
          'chains' => [
            { 'chain' => 'ETH', 'isDefault' => 'true', 'withdrawFee' => '0.005' },
            { 'chain' => 'BSC', 'isDefault' => 'false', 'withdrawFee' => '0.0001' }
          ]
        }
      ]
    }

    Clients::Bitget.any_instance
                   .stubs(:get_coins)
                   .returns(Result::Success.new(api_response))

    result = exchange.fetch_withdrawal_fees!

    assert result.success?
    ea.reload
    assert_equal '0.005', ea.withdrawal_fee
    assert_not_nil ea.withdrawal_fee_updated_at

    # Verify chain data is persisted
    chains = ea.withdrawal_chains
    assert_equal 2, chains.size
    assert_equal 'ETH', chains[0]['name']
    assert_equal true, chains[0]['is_default']
    assert_equal 'BSC', chains[1]['name']
    assert_equal false, chains[1]['is_default']
  end

  # update_exchange_asset_fees! without chains

  test 'update_exchange_asset_fees! without chains does not overwrite existing chain data' do
    @ea.update!(withdrawal_chains: [{ 'name' => 'BTC', 'fee' => '0.0005', 'is_default' => true }])

    @exchange.send(:update_exchange_asset_fees!, { @asset.symbol => '0.001' })

    @ea.reload
    assert_equal '0.001', @ea.withdrawal_fee
    # Chain data preserved — not overwritten when chains param omitted
    assert_equal 1, @ea.withdrawal_chains.size
    assert_equal 'BTC', @ea.withdrawal_chains.first['name']
  end

  # Coinbase stub test

  test 'Coinbase fetch_withdrawal_fees! returns empty success' do
    Rails.configuration.stubs(:dry_run).returns(false)

    exchange = create(:coinbase_exchange)
    result = exchange.fetch_withdrawal_fees!

    assert result.success?
    assert_equal({}, result.data)
  end

  # Kraken stub test

  test 'Kraken fetch_withdrawal_fees! returns empty success' do
    Rails.configuration.stubs(:dry_run).returns(false)

    exchange = create(:kraken_exchange)
    result = exchange.fetch_withdrawal_fees!

    assert result.success?
    assert_equal({}, result.data)
  end
end
