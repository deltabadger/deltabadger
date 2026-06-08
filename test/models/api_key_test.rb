require 'test_helper'

class ApiKeyTest < ActiveSupport::TestCase
  test 'stores and encrypts the IBKR OAuth credential fields' do
    api_key = create(:api_key,
                     access_token: 'tok-123',
                     rsa_signature_key: "-----BEGIN PRIVATE KEY-----\nsig\n-----END PRIVATE KEY-----",
                     rsa_encryption_key: "-----BEGIN PRIVATE KEY-----\nenc\n-----END PRIVATE KEY-----",
                     dh_param: "-----BEGIN DH PARAMETERS-----\ndh\n-----END DH PARAMETERS-----",
                     ibkr_realm: 'limited_poa')

    api_key.reload
    assert_equal 'tok-123', api_key.access_token
    assert_includes api_key.rsa_signature_key, 'sig'
    assert_includes api_key.rsa_encryption_key, 'enc'
    assert_includes api_key.dh_param, 'DH PARAMETERS'
    assert_equal 'limited_poa', api_key.ibkr_realm

    # encrypted at rest: the raw column must not contain the plaintext
    raw = ApiKey.connection.select_value("SELECT access_token FROM api_keys WHERE id = #{api_key.id}")
    assert raw.present?
    refute_equal 'tok-123', raw
  end

  test 'pending_activation is a valid status appended without shifting existing ones' do
    assert_equal 0, ApiKey.statuses['pending_validation']
    assert_equal 1, ApiKey.statuses['correct']
    assert_equal 2, ApiKey.statuses['incorrect']
    assert_equal 3, ApiKey.statuses['pending_activation']

    api_key = create(:api_key)
    api_key.update!(status: :pending_activation)
    assert_predicate api_key.reload, :pending_activation?
  end

  test 'validate_credentials! maps a :pending_activation validity to pending_activation and persists the creds' do
    api_key = create(:api_key, :pending)
    api_key.exchange.stubs(:get_api_key_validity).returns(Result::Success.new(:pending_activation))

    api_key.validate_credentials!(key: 'CONSUMER1', secret: 'sekret', access_token: 'tok', ibkr_realm: 'limited_poa')

    api_key.reload
    assert_predicate api_key, :pending_activation?
    assert_equal 'tok', api_key.access_token
    assert_equal 'limited_poa', api_key.ibkr_realm
  end

  test 'validate_credentials! still maps a truthy validity to correct (regression)' do
    api_key = create(:api_key, :pending)
    api_key.exchange.stubs(:get_api_key_validity).returns(Result::Success.new(true))

    api_key.validate_credentials!(key: 'k', secret: 's')

    assert_predicate api_key.reload, :correct?
  end

  test 'validate_credentials! still maps a falsey validity to incorrect (regression)' do
    api_key = create(:api_key, :pending)
    api_key.exchange.stubs(:get_api_key_validity).returns(Result::Success.new(false))

    api_key.validate_credentials!(key: 'k', secret: 's')

    assert_predicate api_key, :incorrect?
  end

  test 'stop_dependent_bots! stops only the working bots on the key exchange' do
    api_key = create(:api_key) # binance, trading
    user = api_key.user
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    create(:dca_single_asset, :started, user: user, exchange: api_key.exchange, base_asset: btc, quote_asset: usd)
    create(:dca_single_asset, :started, user: user, exchange: create(:kraken_exchange), base_asset: eth, quote_asset: usd)

    # Exactly the one bot on the key's exchange must be stopped (stub stop to dodge the
    # Accountable save guard that fires on a freshly-loaded bot in tests).
    Bots::DcaSingleAsset.any_instance.expects(:stop).once

    api_key.stop_dependent_bots!
  end
end
