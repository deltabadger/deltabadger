require 'test_helper'

# Starting a bot that can sell is the loudest "I am about to trade" there is, and it happens on the
# bots index as often as on a bot's own page. The question has to be answered first — not asked
# afterwards, when the bot is already scheduled.
class Bots::StartWashSaleTest < ActionDispatch::IntegrationTest
  TURBO_STREAM_ACCEPT = 'text/vnd.turbo-stream.html, text/html'.freeze

  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    sign_in @user
    @buyer = create(:dca_single_asset, user: @user)
    @seller = create(:dca_single_asset, user: @user, exchange: @buyer.exchange,
                                        base_asset: Asset.find_by(symbol: 'BTC'),
                                        quote_asset: Asset.find_by(symbol: 'USD'))
    @seller.update_column(:settings, @seller.settings.merge('direction' => 'selling'))

    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    Bots::DcaSingleAsset.any_instance.stubs(:check_missed_quote_amount_was_set).returns(true)
  end

  # `restarting?` is stopped + a past run, and last_action_job_at is a transient_data accessor.
  def stopped_after_a_run(bot)
    bot.update_columns(status: Bot.statuses[:stopped],
                       transient_data: bot.transient_data.merge('last_action_job_at' => 1.hour.ago))
  end

  def start(bot)
    patch bot_start_path(bot_id: bot.id, start_fresh: true), headers: { 'Accept' => TURBO_STREAM_ACCEPT }
  end

  test 'a bot that can sell will not start while the question is unanswered' do
    start(@seller)

    assert_response :unprocessable_entity
    assert_not_predicate @seller.reload, :scheduled?
    assert_match 'wash-sale-question', response.body, 'and the question comes back with the refusal'
  end

  test 'the refusal is the question, not a dead end' do
    start(@seller)

    assert_match 'wash_sale[enabled]', response.body
    assert_match 'target="modal"', response.body,
                 'the bots index has no start template of its own, only the layout modal frame'
  end

  test 'answering once is enough — the next start goes through' do
    @user.update!(wash_sale_enabled: false)

    start(@seller)

    assert_response :success
    assert_predicate @seller.reload, :scheduled?
  end

  test 'a buy-only bot is not held up by a tax question about selling' do
    start(@buyer)

    assert_response :success
    assert_predicate @buyer.reload, :scheduled?
  end

  test 'the restart question is replaced by the wash-sale question while it is due' do
    stopped_after_a_run(@seller)

    get edit_bot_start_path(bot_id: @seller.id)

    assert_response :success
    assert_select '.wash-sale-question'
    # The start question must not be reachable behind the one that gates it.
    assert_select 'form[action=?]', bot_start_path(bot_id: @seller.id), count: 0
  end

  test 'once answered, the restart question is the restart question again' do
    @user.update!(wash_sale_enabled: false)
    stopped_after_a_run(@seller)

    get edit_bot_start_path(bot_id: @seller.id)

    assert_select '.wash-sale-question', count: 0
    assert_select 'form[action^=?]', bot_start_path(bot_id: @seller.id)
  end
end
