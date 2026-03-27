require 'test_helper'

class Exchanges::BinanceTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    Rails.configuration.stubs(:dry_run).returns(false)
  end

  def valid_api_description(trading: false, withdrawal: false)
    {
      'ipRestrict' => true,
      'enableFixApiTrade' => false,
      'enableFixReadOnly' => false,
      'enableFutures' => false,
      'enableInternalTransfer' => false,
      'enableMargin' => false,
      'enablePortfolioMarginTrading' => false,
      'enableReading' => true,
      'enableSpotAndMarginTrading' => trading,
      'enableVanillaOptions' => false,
      'enableWithdrawals' => withdrawal,
      'permitsUniversalTransfer' => false
    }
  end

  test 'get_api_key_validity validates trading key permissions' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Binance.any_instance.stubs(:api_description).returns(
      Result::Success.new(valid_api_description(trading: true, withdrawal: false))
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity rejects trading key with withdrawal permissions' do
    api_key = create(:api_key, exchange: @exchange, key_type: :trading, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Binance.any_instance.stubs(:api_description).returns(
      Result::Success.new(valid_api_description(trading: false, withdrawal: true))
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity validates withdrawal key permissions' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Binance.any_instance.stubs(:api_description).returns(
      Result::Success.new(valid_api_description(trading: false, withdrawal: true))
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal true, result.data
  end

  test 'get_api_key_validity rejects withdrawal key with trading permissions' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'test_key', secret: 'test_secret')

    Honeymaker::Clients::Binance.any_instance.stubs(:api_description).returns(
      Result::Success.new(valid_api_description(trading: true, withdrawal: false))
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end

  test 'get_api_key_validity returns false for invalid key' do
    api_key = create(:api_key, exchange: @exchange, key_type: :withdrawal, key: 'bad_key', secret: 'bad_secret')

    Honeymaker::Clients::Binance.any_instance.stubs(:api_description).returns(
      Result::Failure.new('{"code":-2015,"msg":"Invalid API-key, IP, or permissions for action."}')
    )

    result = @exchange.get_api_key_validity(api_key: api_key)
    assert result.success?
    assert_equal false, result.data
  end
end
