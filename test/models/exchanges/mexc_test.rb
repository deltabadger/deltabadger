require 'test_helper'

class Exchanges::MexcTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:mexc_exchange)
  end

  test 'coingecko_id returns mxc' do
    assert_equal 'mxc', @exchange.coingecko_id
  end

  test 'known_errors returns expected error messages' do
    errors = @exchange.known_errors
    assert errors[:insufficient_funds].present?
    assert errors[:invalid_key].present?
    assert_includes errors[:insufficient_funds], 'Insufficient balance.'
    assert_includes errors[:invalid_key], 'Invalid Api-Key ID.'
  end

  test 'transient_error? treats timestamp/recvWindow rejections as transient' do
    assert @exchange.transient_error?(['Timestamp for this request is outside of the recvWindow.'])
  end

  test 'minimum_amount_logic returns base_and_quote for market orders' do
    assert_equal :base_and_quote, @exchange.minimum_amount_logic(order_type: :market_order)
  end

  test 'minimum_amount_logic returns base_and_quote_in_base for limit orders' do
    assert_equal :base_and_quote_in_base, @exchange.minimum_amount_logic(order_type: :limit_order)
  end

  test 'set_client creates a Honeymaker::Clients::Mexc instance' do
    @exchange.set_client
    assert_kind_of Honeymaker::Clients::Mexc, @exchange.send(:client)
  end

  test 'set_client with api_key stores the api_key' do
    api_key = create(:api_key, exchange: @exchange)
    @exchange.set_client(api_key: api_key)
    assert_equal api_key, @exchange.api_key
  end

  test 'requires_passphrase? returns false' do
    assert_equal false, @exchange.requires_passphrase?
  end

  test 'get_api_key_validity checks canTrade for trading keys' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Mexc.any_instance.stubs(:account_information).returns(
      Result::Success.new({ 'canTrade' => true, 'canWithdraw' => false, 'balances' => [] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity rejects trading key without canTrade' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Mexc.any_instance.stubs(:account_information).returns(
      Result::Success.new({ 'canTrade' => false, 'canWithdraw' => true, 'balances' => [] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity accepts withdrawal key with successful account_information' do
    Rails.configuration.stubs(:dry_run).returns(false)
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Mexc.any_instance.stubs(:account_information).returns(
      Result::Success.new({ 'canTrade' => false, 'canWithdraw' => true, 'balances' => [] })
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end
  # B8. The container has its own MEXC parser (data-api uses honeymaker's; the container cannot,
  # because honeymaker's connection takes no proxy and would put the container's own IP at MEXC).
  # Two implementations of one contract, so both are driven from the SAME captured bytes and both
  # assert its digest — a hand-edit in either repo would otherwise let them drift again.
  def mexc_exchange_info
    JSON.parse(File.read(Rails.root.join('test/fixtures/exchanges/mexc_exchange_info.json')))
  end

  test 'the MEXC fixture is the same bytes honeymaker tests against' do
    path = Rails.root.join('test/fixtures/exchanges/mexc_exchange_info.json')
    assert_equal '13a65c0c8ebde52fac4cdca9a5be9d9a18ba7b5438c496b1b30606cf52fd60d3',
                 Digest::SHA256.hexdigest(File.read(path))
    assert_equal '1', mexc_exchange_info['symbols'].first['status']
  end

  test 'get_tickers_info reads MEXC status and spot permission' do
    @exchange.set_client
    @exchange.send(:client).stubs(:exchange_information).returns(Result::Success.new(mexc_exchange_info))

    result = @exchange.get_tickers_info(force: true)

    assert result.success?
    allowed = result.data.find { |t| t[:ticker] == 'METALUSDT' }
    disallowed = result.data.find { |t| t[:ticker] == 'PALMAIUSDT' }
    assert allowed[:trading_enabled], 'status "1" plus spot permission is tradable'
    assert disallowed[:available], 'still listed'
    assert_not disallowed[:trading_enabled], 'MEXC rejects a spot order for this symbol'
  end
end
