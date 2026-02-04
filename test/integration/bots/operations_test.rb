require "test_helper"

class Bots::OperationsTest < ActionDispatch::IntegrationTest
  setup do
    @admin = create(:user, admin: true)
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @bitcoin = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @ticker = create(:ticker, exchange: @exchange, base_asset: @bitcoin, quote_asset: @usd)
    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
    @bot = create(:dca_single_asset,
      user: @user,
      exchange: @exchange,
      base_asset: @bitcoin,
      quote_asset: @usd,
      status: :stopped,
      with_api_key: false)

    sign_in @user
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    Bots::DcaSingleAsset.any_instance.stubs(:check_missed_quote_amount_was_set).returns(true)
  end

  # == Starting a bot ==

  test "starts a stopped bot" do
    patch bot_start_path(bot_id: @bot.id), params: {start_fresh: "true"}, as: :turbo_stream

    assert_response :ok
    assert_predicate @bot.reload, :scheduled?
    assert @bot.started_at.present?
  end

  test "starting fresh resets missed amount" do
    @bot.update!(missed_quote_amount: 50)

    patch bot_start_path(bot_id: @bot.id), params: {start_fresh: "true"}, as: :turbo_stream

    assert_response :ok
    assert_equal 0, @bot.reload.missed_quote_amount
  end

  test "starting schedules the action job" do
    Bot::ActionJob.expects(:perform_later).with(@bot)

    patch bot_start_path(bot_id: @bot.id), params: {start_fresh: "true"}, as: :turbo_stream
  end

  test "starting returns error when ticker unavailable" do
    @ticker.update!(available: false)

    patch bot_start_path(bot_id: @bot.id), params: {start_fresh: "true"}, as: :turbo_stream

    assert_response :unprocessable_content
    assert_predicate @bot.reload, :stopped?
  end

  # == Stopping a bot ==

  test "stops a running bot" do
    @bot.update!(status: :scheduled, started_at: Time.current)

    patch bot_stop_path(bot_id: @bot.id), as: :turbo_stream

    assert_response :ok
    assert_predicate @bot.reload, :stopped?
    assert @bot.stopped_at.present?
  end

  # == Deleting a bot ==

  test "soft-deletes a bot" do
    delete bot_delete_path(bot_id: @bot.id), as: :turbo_stream

    assert_response :ok
    assert_predicate @bot.reload, :deleted?
  end

  # == Authorization ==

  test "prevents accessing another user's bot" do
    other_user = create(:user)
    create(:api_key, user: other_user, exchange: @exchange, key_type: :trading, status: :correct)
    other_bot = create(:dca_single_asset,
      user: other_user,
      exchange: @exchange,
      base_asset: @bitcoin,
      quote_asset: @usd,
      with_api_key: false)

    patch bot_start_path(bot_id: other_bot.id), as: :turbo_stream

    assert_redirected_to bots_path
  end

  test "prevents deleting another user's bot" do
    other_user = create(:user)
    create(:api_key, user: other_user, exchange: @exchange, key_type: :trading, status: :correct)
    other_bot = create(:dca_single_asset,
      user: other_user,
      exchange: @exchange,
      base_asset: @bitcoin,
      quote_asset: @usd,
      with_api_key: false)

    delete bot_delete_path(bot_id: other_bot.id), as: :turbo_stream

    assert_redirected_to bots_path
    assert_not_predicate other_bot.reload, :deleted?
  end

  # == Authentication ==

  test "requires login to start bot" do
    sign_out @user

    patch bot_start_path(bot_id: @bot.id), as: :turbo_stream
    assert_redirected_to new_user_session_path
  end

  test "requires login to delete bot" do
    sign_out @user

    delete bot_delete_path(bot_id: @bot.id), as: :turbo_stream
    assert_redirected_to new_user_session_path
  end
end
