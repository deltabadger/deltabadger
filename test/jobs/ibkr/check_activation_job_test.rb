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

  test 'leaves a still-not-activated key pending, without resetting its clock' do
    Clients::Ibkr.any_instance.stubs(:accounts).returns(Result::Failure.new('not authenticated'))
    submitted_at = 20.days.ago
    @api_key.update_column(:updated_at, submitted_at)

    Ibkr::CheckActivationJob.new.perform

    assert_predicate @api_key.reload, :pending_activation?
    # The wizard reads updated_at as "when the user last submitted credentials" and fails the
    # connection out once it is older than ApiKey::ACTIVATION_DEADLINE. A failed poll every 6h
    # must never touch it, or a dead registration would look fresh forever.
    assert_in_delta submitted_at, @api_key.updated_at, 1.second
  end

  test 'only considers IBKR keys (ignores other exchanges)' do
    other = create(:api_key, exchange: create(:binance_exchange), user: create(:user), status: :pending_activation)
    Clients::Ibkr.any_instance.stubs(:accounts).returns(Result::Success.new({ 'accounts' => ['U1'] }))

    Ibkr::CheckActivationJob.new.perform

    assert_predicate other.reload, :pending_activation?
  end
end
