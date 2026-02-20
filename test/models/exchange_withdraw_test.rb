require 'test_helper'

class ExchangeWithdrawTest < ActiveSupport::TestCase
  # Tests that exchanges skip chain-lookup API calls when network is explicitly provided,
  # and correctly pass network/address_tag through to clients.

  setup do
    Rails.configuration.stubs(:dry_run).returns(false)
    @asset = create(:asset, :bitcoin)
    @quote_asset = create(:asset, :usd)
  end

  # Bybit

  test 'Bybit withdraw skips get_coin_query_info when network is provided' do
    exchange = create(:bybit_exchange)
    create(:ticker, exchange: exchange, base_asset: @asset, quote_asset: @quote_asset)
    create(:api_key, exchange: exchange, key_type: :withdrawal)

    Clients::Bybit.any_instance.expects(:get_coin_query_info).never
    Clients::Bybit.any_instance.expects(:withdraw).with(
      coin: @asset.symbol, chain: 'BEP20', address: 'addr1',
      amount: '0.5', tag: 'memo1'
    ).returns(Result::Success.new({ 'result' => { 'id' => 'bybit-123' } }))

    result = exchange.withdraw(asset: @asset, amount: 0.5, address: 'addr1',
                               network: 'BEP20', address_tag: 'memo1')
    assert result.success?
  end

  test 'Bybit withdraw fetches default chain when network is nil' do
    exchange = create(:bybit_exchange)
    create(:ticker, exchange: exchange, base_asset: @asset, quote_asset: @quote_asset)
    create(:api_key, exchange: exchange, key_type: :withdrawal)

    coin_response = {
      'result' => {
        'rows' => [{
          'coin' => @asset.symbol,
          'chains' => [
            { 'chain' => 'BTC', 'chainDefault' => '1' },
            { 'chain' => 'BEP20', 'chainDefault' => '0' }
          ]
        }]
      }
    }

    Clients::Bybit.any_instance.expects(:get_coin_query_info)
                  .returns(Result::Success.new(coin_response))
    Clients::Bybit.any_instance.expects(:withdraw).with(
      coin: @asset.symbol, chain: 'BTC', address: 'addr1',
      amount: '0.5', tag: nil
    ).returns(Result::Success.new({ 'result' => { 'id' => 'bybit-456' } }))

    result = exchange.withdraw(asset: @asset, amount: 0.5, address: 'addr1')
    assert result.success?
  end

  # Bitget

  test 'Bitget withdraw skips get_coins when network is provided' do
    exchange = create(:bitget_exchange)
    create(:ticker, exchange: exchange, base_asset: @asset, quote_asset: @quote_asset)
    create(:api_key, exchange: exchange, key_type: :withdrawal)

    Clients::Bitget.any_instance.expects(:get_coins).never
    Clients::Bitget.any_instance.expects(:withdraw).with(
      coin: @asset.symbol, address: 'addr1', size: '0.5',
      chain: 'ERC20', tag: 'tag1'
    ).returns(Result::Success.new({ 'data' => { 'orderId' => 'bg-123' } }))

    result = exchange.withdraw(asset: @asset, amount: 0.5, address: 'addr1',
                               network: 'ERC20', address_tag: 'tag1')
    assert result.success?
  end

  # KuCoin

  test 'KuCoin withdraw skips get_currencies when network is provided' do
    exchange = create(:kucoin_exchange)
    create(:ticker, exchange: exchange, base_asset: @asset, quote_asset: @quote_asset)
    create(:api_key, exchange: exchange, key_type: :withdrawal)

    Clients::Kucoin.any_instance.expects(:get_currencies).never
    Clients::Kucoin.any_instance.expects(:withdraw).with(
      currency: @asset.symbol, address: 'addr1', amount: '0.5',
      chain: 'TRC20', memo: 'memo1'
    ).returns(Result::Success.new({ 'data' => { 'withdrawalId' => 'kc-123' } }))

    result = exchange.withdraw(asset: @asset, amount: 0.5, address: 'addr1',
                               network: 'TRC20', address_tag: 'memo1')
    assert result.success?
  end

  # MEXC

  test 'MEXC withdraw skips get_all_coins_information when network is provided' do
    exchange = create(:mexc_exchange)
    create(:ticker, exchange: exchange, base_asset: @asset, quote_asset: @quote_asset)
    create(:api_key, exchange: exchange, key_type: :withdrawal)

    Clients::Mexc.any_instance.expects(:get_all_coins_information).never
    Clients::Mexc.any_instance.expects(:withdraw).with(
      coin: @asset.symbol, address: 'addr1', amount: '0.5',
      network: 'BEP20', memo: 'memo1'
    ).returns(Result::Success.new({ 'id' => 'mexc-123' }))

    result = exchange.withdraw(asset: @asset, amount: 0.5, address: 'addr1',
                               network: 'BEP20', address_tag: 'memo1')
    assert result.success?
  end

  # Binance — passes network and address_tag directly

  test 'Binance withdraw passes network and address_tag to client' do
    exchange = create(:binance_exchange)
    create(:ticker, exchange: exchange, base_asset: @asset, quote_asset: @quote_asset)
    create(:api_key, exchange: exchange, key_type: :withdrawal)

    Clients::Binance.any_instance.expects(:withdraw).with(
      coin: @asset.symbol, address: 'addr1', amount: '0.5',
      network: 'BSC', address_tag: 'tag1'
    ).returns(Result::Success.new({ 'id' => 'bn-123' }))

    result = exchange.withdraw(asset: @asset, amount: 0.5, address: 'addr1',
                               network: 'BSC', address_tag: 'tag1')
    assert result.success?
  end
end
