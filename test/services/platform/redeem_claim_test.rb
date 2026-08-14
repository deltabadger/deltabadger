require 'test_helper'

class Platform::RedeemClaimTest < ActiveSupport::TestCase
  setup do
    @code = 'dbc_instance_secret'
    @client = mock('launchpad_client')
    Clients::Launchpad.stubs(:new).returns(@client)
  end

  test 'persists claimed market data and generic proxies then starts the initial sync' do
    travel_to Time.utc(2026, 8, 14, 12, 0, 0) do
      @client.expects(:claim).with(@code).returns(Result::Success.new(claim_payload))
      Setup::SeedAndSyncJob.expects(:perform_later).once

      result = Platform::RedeemClaim.call(code: @code)

      assert_predicate result, :success?
      assert_equal({ email: 'owner@example.com', name: 'Owner' }, result.data)
      assert_equal 'https://market-data.example.com', AppConfig.market_data_url
      assert_equal 'dbi_token', AppConfig.market_data_token
      assert_equal MarketDataSettings::PROVIDER_DELTABADGER, AppConfig.market_data_provider
      assert_equal 'https://binance-proxy.example.com', AppConfig.get('proxy_binance')
      assert_equal 'https://kraken-proxy.example.com', AppConfig.get('proxy_kraken')
      assert_equal '2026-08-14T12:00:00Z', AppConfig.get('platform_connected_at')
      assert_equal AppConfig::SYNC_STATUS_PENDING, AppConfig.setup_sync_status
    end
  end

  test 'maps a 401 to a user-presentable failure without writing configuration' do
    response = Result::Failure.new('{"error":"invalid_or_expired"}', data: { status: 401 })
    @client.expects(:claim).with(@code).returns(response)
    Setup::SeedAndSyncJob.expects(:perform_later).never

    assert_no_difference 'AppConfig.count' do
      result = Platform::RedeemClaim.call(code: @code)

      assert_predicate result, :failure?
      assert_equal ['That claim code is invalid or has expired.'], result.errors
    end
  end

  test 'passes a timeout failure through without writing configuration' do
    timeout = Result::Failure.new('Unable to reach Deltabadger. Please try again.')
    @client.expects(:claim).with(@code).returns(timeout)
    Setup::SeedAndSyncJob.expects(:perform_later).never

    assert_no_difference 'AppConfig.count' do
      assert_same timeout, Platform::RedeemClaim.call(code: @code)
    end
  end

  test 'rejects a blank market data token without writing configuration' do
    payload = claim_payload
    payload[:market_data][:token] = ''
    @client.expects(:claim).with(@code).returns(Result::Success.new(payload))
    ApplicationRecord.expects(:transaction).never
    Setup::SeedAndSyncJob.expects(:perform_later).never

    assert_no_difference 'AppConfig.count' do
      result = Platform::RedeemClaim.call(code: @code)

      assert_predicate result, :failure?
      assert_equal ['Deltabadger returned incomplete market data configuration. Please try again.'], result.errors
    end
    assert_empty AppConfig.where(key: claim_config_keys)
  end

  test 'accepts null proxies and persists the rest of the claim' do
    payload = claim_payload.merge(proxies: nil)
    @client.expects(:claim).with(@code).returns(Result::Success.new(payload))
    Setup::SeedAndSyncJob.expects(:perform_later).once

    result = Platform::RedeemClaim.call(code: @code)

    assert_predicate result, :success?
    assert_equal 'https://market-data.example.com', AppConfig.market_data_url
    assert_equal 'dbi_token', AppConfig.market_data_token
    assert_equal MarketDataSettings::PROVIDER_DELTABADGER, AppConfig.market_data_provider
    assert_nil AppConfig.get('proxy_binance')
    assert_nil AppConfig.get('proxy_kraken')
  end

  test 'rolls back every configuration write when a later write fails' do
    @client.expects(:claim).with(@code).returns(Result::Success.new(claim_payload))
    AppConfig.expects(:market_data_provider=)
             .with(MarketDataSettings::PROVIDER_DELTABADGER)
             .raises(StandardError, 'injected write failure')
    Setup::SeedAndSyncJob.expects(:perform_later).never

    error = assert_raises(StandardError) do
      Platform::RedeemClaim.call(code: @code)
    end

    assert_equal 'injected write failure', error.message
    assert_empty AppConfig.where(key: claim_config_keys)
  end

  private

  def claim_payload
    {
      identity: { email: 'owner@example.com', name: 'Owner' },
      market_data: {
        url: 'https://market-data.example.com',
        token: 'dbi_token',
        provider_name: 'Deltabadger'
      },
      proxies: {
        Binance: 'https://binance-proxy.example.com',
        'KRAKEN' => 'https://kraken-proxy.example.com'
      }
    }
  end

  def claim_config_keys
    %w[
      market_data_url
      market_data_token
      market_data_provider
      proxy_binance
      proxy_kraken
      platform_connected_at
      setup_sync_status
    ]
  end
end
