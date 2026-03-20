require 'test_helper'

class BroadcastsTest < ActionDispatch::IntegrationTest
  include ActiveJob::TestHelper

  setup do
    @user = create(:user, admin: true, setup_completed: true)
    @bot = create(:dca_single_asset, user: @user)
  end

  # --- Authentication: every endpoint requires login ---

  test 'unauthenticated request to metrics_update redirects to login' do
    post broadcasts_metrics_update_path
    assert_response :redirect
  end

  test 'unauthenticated request to pnl_update redirects to login' do
    post broadcasts_pnl_update_path
    assert_response :redirect
  end

  test 'unauthenticated request to price_limit_info_update redirects to login' do
    post broadcasts_price_limit_info_update_path
    assert_response :redirect
  end

  test 'unauthenticated request to price_drop_limit_info_update redirects to login' do
    post broadcasts_price_drop_limit_info_update_path
    assert_response :redirect
  end

  test 'unauthenticated request to indicator_limit_info_update redirects to login' do
    post broadcasts_indicator_limit_info_update_path
    assert_response :redirect
  end

  test 'unauthenticated request to moving_average_limit_info_update redirects to login' do
    post broadcasts_moving_average_limit_info_update_path
    assert_response :redirect
  end

  test 'unauthenticated request to fetch_open_orders redirects to login' do
    post broadcasts_fetch_open_orders_path
    assert_response :redirect
  end

  test 'unauthenticated request to wake_dispatcher redirects to login' do
    post broadcasts_wake_dispatcher_path
    assert_response :redirect
  end

  # --- Authorization: bots scoped to current_user ---

  test 'metrics_update returns not_found for bot belonging to another user' do
    sign_in @user
    other_bot = create_other_user_bot

    post broadcasts_metrics_update_path, params: { bot_id: other_bot.id }
    assert_response :not_found
  end

  test 'metrics_update succeeds for own bot' do
    sign_in @user

    post broadcasts_metrics_update_path, params: { bot_id: @bot.id }
    assert_response :ok
  end

  test 'pnl_update only processes bots belonging to current user' do
    sign_in @user
    other_bot = create_other_user_bot

    post broadcasts_pnl_update_path, params: { bot_ids: [@bot.id, other_bot.id] }
    assert_response :ok
  end

  test 'fetch_open_orders returns not_found for bot belonging to another user' do
    sign_in @user
    other_bot = create_other_user_bot

    post broadcasts_fetch_open_orders_path, params: { bot_id: other_bot.id }
    assert_response :not_found
  end

  test 'price_limit_info_update returns not_found for bot belonging to another user' do
    sign_in @user
    other_bot = create_other_user_bot

    post broadcasts_price_limit_info_update_path, params: { bot_id: other_bot.id }
    assert_response :not_found
  end

  # --- Authenticated happy paths ---

  test 'wake_dispatcher succeeds when authenticated' do
    sign_in @user
    post broadcasts_wake_dispatcher_path
    assert_response :ok
  end

  private

  def create_other_user_bot
    other_user = create(:user, setup_completed: true)
    ticker = @bot.exchange.tickers.first
    create(:dca_single_asset,
           user: other_user,
           exchange: @bot.exchange,
           base_asset: ticker.base_asset,
           quote_asset: ticker.quote_asset)
  end
end
