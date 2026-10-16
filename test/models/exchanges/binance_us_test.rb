require 'test_helper'

class Exchanges::BinanceUsTest < ActiveSupport::TestCase
  # What api.binance.us answers for a path it does not have (apiRestrictions, convert, dust, earn, ...).
  NOT_FOUND = Result::Failure.new('{"timestamp":1792147874132,"status":404,"error":"Not Found",' \
                                  '"message":"No message available","path":"/sapi/v1/account/apiRestrictions"}',
                                  data: { status: 404 })
  UNKNOWN_ORDER = Result::Failure.new('{"code":-2011,"msg":"Unknown order sent."}', data: { status: 400 })
  NO_PERMISSION = Result::Failure.new('{"code":-2015,"msg":"Invalid API-key, IP, or permissions for action."}',
                                      data: { status: 400 })
  ACCOUNT = { 'canTrade' => true, 'canWithdraw' => true, 'canDeposit' => true, 'accountType' => 'SPOT',
              'balances' => [{ 'asset' => 'BTC', 'free' => '0.1', 'locked' => '0' }], 'permissions' => ['SPOT'] }.freeze

  setup do
    @exchange = create(:binance_us_exchange)
    Rails.configuration.stubs(:dry_run).returns(false)
    Honeymaker::Clients::BinanceUs.any_instance.stubs(:api_description).returns(NOT_FOUND)
  end

  def key_validity(key_type:, account: Result::Success.new(ACCOUNT), probe: UNKNOWN_ORDER)
    Honeymaker::Clients::BinanceUs.any_instance.stubs(:account_information).returns(account)
    Honeymaker::Clients::BinanceUs.any_instance.stubs(:cancel_order)
                                  .with(symbol: 'ETHBTC', order_id: '9999999999').returns(probe)
    connection = create(:api_key, exchange: @exchange, key_type: key_type, key: 'test_key', secret: 'test_secret')
    @exchange.get_api_key_validity(api_key: connection)
  end

  # --- keys: Binance.US has no apiRestrictions; a key is judged by what it can do ---

  test 'a trading key that reads and may trade is valid' do
    result = key_validity(key_type: :trading)
    assert result.success?
    assert_equal true, result.data
  end

  test 'a trading key that may not trade is refused' do
    assert_equal false, key_validity(key_type: :trading, probe: NO_PERMISSION).data
  end

  test 'a withdrawal key that may not trade is valid' do
    assert_equal true, key_validity(key_type: :withdrawal, probe: NO_PERMISSION).data
  end

  test 'a withdrawal key that may trade is refused' do
    assert_equal false, key_validity(key_type: :withdrawal).data
  end

  test 'an unknown key is refused' do
    result = key_validity(key_type: :trading,
                          account: Result::Failure.new('{"code":-2008,"msg":"Invalid Api-Key ID."}', data: { status: 400 }))
    assert result.success?
    assert_equal false, result.data
  end

  test 'a probe that never answered is a failure, not a verdict' do
    timeout = Result::Failure.new('Net::ReadTimeout', data: { status: nil })
    assert_same timeout, key_validity(key_type: :trading, probe: timeout)
  end

  test 'a reading key that reads is valid' do
    Honeymaker::Clients::BinanceUs.any_instance.stubs(:account_information).returns(Result::Success.new(ACCOUNT))
    connection = create(:api_key, exchange: @exchange, key_type: :read_only, key: 'test_key', secret: 'test_secret')

    result = @exchange.get_read_api_key_validity(api_key: connection)
    assert result.success?
    assert_equal true, result.data
  end

  test 'the key check is dry in dry-run mode' do
    Rails.configuration.stubs(:dry_run).returns(true)
    Honeymaker.expects(:client).never
    connection = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    assert_equal true, @exchange.get_api_key_validity(api_key: connection).data
  end

  # --- ledger ---

  test 'get_ledger reads through the Binance.US client' do
    connection = create(:api_key, exchange: @exchange)
    hm_client = mock('honeymaker_client')
    hm_client.stubs(:deposit_history).returns(Result::Failure.new('stop'))
    Honeymaker.expects(:client).with('binance_us', has_entries(proxy: ExchangeProxy.for('binance_us'))).returns(hm_client)

    assert @exchange.get_ledger(api_key: connection).failure?
  end
end
