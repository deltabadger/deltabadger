require 'test_helper'

# The two answers to "Redeploy $243?", and clearing the halt when a batch's outcome is unknown.
class Bots::RedeploysControllerTest < ActionDispatch::IntegrationTest
  def setup
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @bot = create(:dca_index, user: @user)
    asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
    create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
    sign_in @user
  end

  test 'Yes queues the work rather than trading inside the request' do
    post bot_redeploy_path(bot_id: @bot.id)

    assert_predicate queued(Bot::RedeployJob), :exists?
  end

  test 'who asked is recorded' do
    post bot_redeploy_path(bot_id: @bot.id)

    log = @bot.bot_activity_logs.find_by(event: 'redeploy_requested')
    assert log, 'a user-initiated placement has to leave a trace'
    assert_equal @user.id, log.details['user_id']
  end

  # Queued, not applied in the request: the offset is a snapshot of a figure a running batch is still
  # moving, so it has to be written under the same semaphore the batch holds.
  test 'No queues the decline rather than writing it in the request' do
    liquidated(150)

    delete bot_redeploy_path(bot_id: @bot.id)

    assert_predicate queued(Bot::DeclineRedeployJob), :exists?
    assert_in_delta 0, @bot.reload.redeploy_declined_offset.to_d.to_f, 0.0001,
                    'nothing is written until the job holds the lock'
  end

  test 'No places nothing' do
    liquidated(150)

    delete bot_redeploy_path(bot_id: @bot.id)

    assert_not_predicate queued(Bot::RedeployJob), :exists?
  end

  test 'No places nothing and reports back' do
    liquidated(150)

    delete bot_redeploy_path(bot_id: @bot.id)

    assert_response :success
    assert_not_predicate queued(Bot::RedeployJob), :exists?
  end

  test 'a bot type with no composition is refused' do
    other = create(:dca_single_asset, user: @user)

    post bot_redeploy_path(bot_id: other.id)

    assert_not_predicate queued(Bot::RedeployJob), :exists?
  end

  test "another user's bot is not reachable" do
    # Not a second :dca_index — two of those collide on the index's external id.
    theirs = create(:dca_single_asset, user: create(:user))

    post bot_redeploy_path(bot_id: theirs.id)

    assert_not_predicate queued(Bot::RedeployJob), :exists?
    assert_empty theirs.bot_activity_logs.where(event: 'redeploy_requested')
  end

  test 'signed out, nothing is queued' do
    sign_out @user

    post bot_redeploy_path(bot_id: @bot.id)

    assert_not_predicate queued(Bot::RedeployJob), :exists?
  end

  # == resolution ==

  test 'the halt is cleared only through a job that holds the semaphore' do
    @bot.start_redeploy_placement!
    @bot.flag_redeploy_ambiguous!

    post bot_redeploy_resolutions_path(bot_id: @bot.id, intent_id: @bot.redeploy_pending[:id])

    assert_predicate queued(Bot::ResolveRedeployJob), :exists?
  end

  test 'a resolution is refused while one of its orders is still open' do
    @bot.start_redeploy_placement!
    @bot.flag_redeploy_ambiguous!
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :open, external_id: 'w-2', side: :buy,
                         transaction_type: 'REDEPLOY', base: 'AAA', quote: @bot.quote_asset.symbol,
                         price: 100, amount: 1, amount_exec: 0, quote_amount: 100, quote_amount_exec: 0)
    Bot::FetchAndUpdateOrderJob.stubs(:perform_now)

    post bot_redeploy_resolutions_path(bot_id: @bot.id, intent_id: @bot.redeploy_pending[:id])

    assert_not_predicate queued(Bot::ResolveRedeployJob), :exists?
  end

  private

  def queued(job_class)
    SolidQueue::Job.where(class_name: job_class.name)
  end

  def liquidated(quote)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :closed, external_id: "l-#{SecureRandom.hex(4)}",
                         side: :sell, transaction_type: 'LIQUIDATION', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: 100, amount: quote.to_d / 100,
                         amount_exec: quote.to_d / 100, quote_amount: quote, quote_amount_exec: quote)
  end
end
