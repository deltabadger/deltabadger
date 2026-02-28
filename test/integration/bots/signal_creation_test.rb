require 'test_helper'

class Bots::SignalCreationTest < ActionDispatch::IntegrationTest
  setup do
    @admin = create(:user, admin: true)
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @bitcoin = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @ticker = create(:ticker, exchange: @exchange, base_asset: @bitcoin, quote_asset: @usd)
    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)

    sign_in @user
  end

  test 'creates a signal bot when completing all wizard steps' do
    # Step 1: Pick buyable asset
    get new_bots_signals_pick_buyable_asset_path
    assert_response :ok

    post bots_signals_pick_buyable_asset_path, params: {
      bots_signal: { base_asset_id: @bitcoin.id }
    }
    assert_redirected_to new_bots_signals_pick_exchange_path
    follow_redirect!

    # Step 2: Pick exchange
    assert_response :ok

    post bots_signals_pick_exchange_path, params: {
      bots_signal: { exchange_id: @exchange.id }
    }
    assert_redirected_to new_bots_signals_add_api_key_path
    follow_redirect!

    # Step 3: API key (already validated, should redirect)
    assert_redirected_to new_bots_signals_pick_spendable_asset_path
    follow_redirect!

    # Step 4: Pick spendable asset
    assert_response :ok

    post bots_signals_pick_spendable_asset_path, params: {
      bots_signal: { quote_asset_id: @usd.id }
    }
    assert_redirected_to new_bots_signals_confirm_settings_path
    follow_redirect!

    # Step 5: Confirm settings
    assert_response :ok

    # Step 6: Create bot
    assert_difference ['Bots::Signal.count', 'BotSignal.count'], 1 do
      post bots_signals_path, as: :turbo_stream
    end

    bot = Bots::Signal.last
    assert_equal @bitcoin, bot.base_asset
    assert_equal @usd, bot.quote_asset
    assert_equal @exchange, bot.exchange
    assert_predicate bot, :scheduled?
    assert_equal 1, bot.bot_signals.count
    assert_predicate bot.bot_signals.first, :buy?
    assert_equal 100, bot.bot_signals.first.amount
  end

  test 'redirects to pick asset when accessing exchange step directly' do
    get new_bots_signals_pick_exchange_path
    assert_redirected_to new_bots_signals_pick_buyable_asset_path
  end

  test 'requires authentication for wizard' do
    sign_out @user

    get new_bots_signals_pick_buyable_asset_path
    assert_redirected_to new_user_session_path
  end

  test 'requires authentication to create bot' do
    sign_out @user

    post bots_signals_path, as: :turbo_stream
    assert_redirected_to new_user_session_path
  end
end
