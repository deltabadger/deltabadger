require 'test_helper'

class ApiKeyValidatorTest < ActiveSupport::TestCase
  setup do
    @original_dry_run = Rails.configuration.dry_run
    Rails.configuration.dry_run = false

    @user = create(:user)
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange, status: :pending_validation)
  end

  teardown do
    Rails.configuration.dry_run = @original_dry_run
  end

  test 'marks api key as correct when validation succeeds' do
    mock_client = mock('honeymaker_client')
    mock_client.expects(:validate).with(:trading).returns(Honeymaker::Result::Success.new(true))
    Honeymaker.expects(:client).with('binance', api_key: @api_key.key, api_secret: @api_key.secret).returns(mock_client)

    result = ApiKeyValidator.call(@api_key.id)

    assert result.success?
    assert_equal 'correct', @api_key.reload.status
  end

  test 'marks api key as incorrect when validation fails' do
    mock_client = mock('honeymaker_client')
    mock_client.expects(:validate).with(:trading).returns(Honeymaker::Result::Failure.new('Invalid key'))
    Honeymaker.expects(:client).with('binance', api_key: @api_key.key, api_secret: @api_key.secret).returns(mock_client)

    result = ApiKeyValidator.call(@api_key.id)

    assert result.failure?
    assert_equal 'incorrect', @api_key.reload.status
  end

  test 'marks api key as incorrect when validation raises error' do
    mock_client = mock('honeymaker_client')
    mock_client.expects(:validate).with(:trading).raises(StandardError, 'Connection timeout')
    Honeymaker.expects(:client).with('binance', api_key: @api_key.key, api_secret: @api_key.secret).returns(mock_client)

    result = ApiKeyValidator.call(@api_key.id)

    assert result.failure?
    assert_equal 'incorrect', @api_key.reload.status
  end

  test 'skips honeymaker validation in dry_run mode' do
    Rails.configuration.dry_run = true
    Honeymaker.expects(:client).never

    result = ApiKeyValidator.call(@api_key.id)

    assert result.success?
    assert_equal 'correct', @api_key.reload.status
  end

  test 'passes passphrase when present' do
    api_key_with_passphrase = create(:api_key, user: @user, exchange: create(:bitget_exchange),
                                               raw_passphrase: 'my_passphrase', status: :pending_validation)

    mock_client = mock('honeymaker_client')
    mock_client.expects(:validate).with(:trading).returns(Honeymaker::Result::Success.new(true))
    Honeymaker.expects(:client).with(
      'bitget',
      api_key: api_key_with_passphrase.key,
      api_secret: api_key_with_passphrase.secret,
      passphrase: 'my_passphrase'
    ).returns(mock_client)

    result = ApiKeyValidator.call(api_key_with_passphrase.id)

    assert result.success?
  end
end
