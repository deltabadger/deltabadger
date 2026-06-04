require 'test_helper'

class Ibkr::CheckActivationJobTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:ibkr_exchange)
    @api_key = create(:api_key, exchange: @exchange, user: create(:user), status: :pending_activation)
    # Exchange::Dryable would otherwise short-circuit get_api_key_validity to true in test env.
    Exchanges::Ibkr.any_instance.stubs(:dry_run?).returns(false)
  end

  test 'flips a pending IBKR key to :correct once IBKR reports it usable' do
    Clients::Ibkr.any_instance.stubs(:accounts).returns(Result::Success.new({ 'accounts' => ['U1'] }))

    Ibkr::CheckActivationJob.new.perform

    assert_predicate @api_key.reload, :correct?
  end

  test 'leaves a still-not-activated key pending' do
    Clients::Ibkr.any_instance.stubs(:accounts).returns(Result::Failure.new('not authenticated'))

    Ibkr::CheckActivationJob.new.perform

    assert_predicate @api_key.reload, :pending_activation?
  end

  test 'only considers IBKR keys (ignores other exchanges)' do
    other = create(:api_key, exchange: create(:binance_exchange), user: create(:user), status: :pending_activation)
    Clients::Ibkr.any_instance.stubs(:accounts).returns(Result::Success.new({ 'accounts' => ['U1'] }))

    Ibkr::CheckActivationJob.new.perform

    assert_predicate other.reload, :pending_activation?
  end
end
