require 'test_helper'

# The legacy POST /api/api_keys path. It runs the same check as every form (ApiKey#get_validity) —
# it used to call honeymaker's own validate(:trading), a third check agreeing with neither the
# steps nor the forms.
class ApiKeyValidatorTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange, status: :pending_validation)
  end

  test 'marks api key as correct when validation succeeds' do
    ApiKey.any_instance.stubs(:get_validity).returns(Result::Success.new(true))

    result = ApiKeyValidator.call(@api_key.id)

    assert result.success?
    assert_equal 'correct', @api_key.reload.status
  end

  test 'marks api key as incorrect when the exchange rejects it' do
    ApiKey.any_instance.stubs(:get_validity).returns(Result::Success.new(false))

    result = ApiKeyValidator.call(@api_key.id)

    assert result.failure?
    assert_equal 'incorrect', @api_key.reload.status
  end

  test 'a key missing a permission is incorrect' do
    ApiKey.any_instance.stubs(:get_validity)
          .returns(Result::Success.new({ missing_permissions: %w[query-ledger], forbidden_permissions: [] }))

    result = ApiKeyValidator.call(@api_key.id)

    assert result.failure?
    assert_equal 'incorrect', @api_key.reload.status
  end

  # An exchange that cannot be asked has not said no.
  test 'a check that cannot complete leaves the key pending' do
    ApiKey.any_instance.stubs(:get_validity).returns(Result::Failure.new('execution expired'))

    result = ApiKeyValidator.call(@api_key.id)

    assert result.failure?
    assert_equal 'pending_validation', @api_key.reload.status
  end

  test 'uses the same check as the forms, never honeymaker validate' do
    ApiKey.any_instance.expects(:get_validity).returns(Result::Success.new(true))
    Honeymaker.expects(:client).never

    ApiKeyValidator.call(@api_key.id)
  end

  test 'a valid trading key starts a balance sync' do
    ApiKey.any_instance.stubs(:get_validity).returns(Result::Success.new(true))
    AccountBalance::SyncJob.expects(:perform_later).with(@user.id, [@api_key.id])

    ApiKeyValidator.call(@api_key.id)
  end
end
