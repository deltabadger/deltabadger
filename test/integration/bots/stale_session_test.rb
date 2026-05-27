require 'test_helper'

class Bots::StaleSessionTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true)
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @bitcoin = create(:asset, :bitcoin)
    @ethereum = create(:asset, :ethereum)
    @usd = create(:asset, :usd)
    @ticker = create(:ticker, exchange: @exchange, base_asset: @bitcoin, quote_asset: @usd)
    create(:ticker, exchange: @exchange, base_asset: @ethereum, quote_asset: @usd)
    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)

    sign_in @user
    Bot::ActionJob.stubs(:perform_later)
    MarketData.stubs(:configured?).returns(true)
    MarketData.stubs(:get_top_coins).returns(Result::Success.new([]))
  end

  # Reproduces the actual production bug: user partially completes Signal wizard
  # (which adds 'signals' to session[:bot_config]), then switches to DcaIndex wizard
  # (which doesn't reset session). DcaIndex.new blows up with UnknownAttributeError.
  # The DcaIndex bot is now persisted at pick_spendable_asset (matches the
  # DCA Single Asset flow), so the failure mode would surface there.
  test 'switching from Signal wizard to DcaIndex wizard does not raise UnknownAttributeError' do
    walk_signal_wizard_to_confirm_settings

    # Switch to DcaIndex wizard (pick_indices uses ||= so session is NOT cleared)
    post bots_dca_indexes_pick_index_path, params: { index_type: 'top' }
    assert_response :redirect

    follow_redirect!
    post bots_dca_indexes_pick_exchange_path, params: {
      bots_dca_index: { exchange_id: @exchange.id }
    }
    follow_redirect!
    follow_redirect! # skip API key (already correct)

    # Picking the spendable asset persists the bot in :created state — no confirm step.
    assert_difference 'Bots::DcaIndex.count', 1 do
      post bots_dca_indexes_pick_spendable_asset_path, params: {
        bots_dca_index: { quote_asset_id: @usd.id }
      }, as: :turbo_stream
    end
    assert_response :ok
    bot = Bots::DcaIndex.last
    assert_predicate bot, :created?
    assert_match %(action="redirect" target="#{bot_path(bot)}"), response.body
  end

  test 'switching from Signal wizard to DCA single asset wizard works cleanly' do
    walk_signal_wizard_to_confirm_settings

    # DCA single asset first step resets session with =, so signals are cleared.
    # But the sanitization still protects the full flow.
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok

    post bots_dca_single_assets_pick_buyable_asset_path, params: {
      bots_dca_single_asset: { base_asset_id: @bitcoin.id }
    }
    follow_redirect!

    post bots_dca_single_assets_pick_exchange_path, params: {
      bots_dca_single_asset: { exchange_id: @exchange.id }
    }
    follow_redirect!
    follow_redirect! # skip API key

    # Picking the spendable asset persists the bot in :created state — no confirm step.
    assert_difference 'Bots::DcaSingleAsset.count', 1 do
      post bots_dca_single_assets_pick_spendable_asset_path, params: {
        bots_dca_single_asset: { quote_asset_id: @usd.id }
      }
    end
  end

  test 'Signal bot creation still works after sanitization changes' do
    walk_signal_wizard_to_confirm_settings

    assert_difference ['Bots::Signal.count', 'BotSignal.count'], 1 do
      post bots_signals_path, as: :turbo_stream
    end

    bot = Bots::Signal.last
    assert_equal @bitcoin, bot.base_asset
    assert_equal @usd, bot.quote_asset
    assert_predicate bot, :scheduled?
    assert_equal 1, bot.bot_signals.count
  end

  private

  def walk_signal_wizard_to_confirm_settings
    get new_bots_signals_pick_buyable_asset_path
    post bots_signals_pick_buyable_asset_path, params: {
      bots_signal: { base_asset_id: @bitcoin.id }
    }
    assert_redirected_to new_bots_signals_pick_exchange_path
    follow_redirect!

    post bots_signals_pick_exchange_path, params: {
      bots_signal: { exchange_id: @exchange.id }
    }
    assert_redirected_to new_bots_signals_add_api_key_path
    follow_redirect!

    # API key is already correct, so this redirects to pick_spendable_asset
    assert_redirected_to new_bots_signals_pick_spendable_asset_path
    follow_redirect!

    post bots_signals_pick_spendable_asset_path, params: {
      bots_signal: { quote_asset_id: @usd.id }
    }
    assert_redirected_to new_bots_signals_confirm_settings_path
    follow_redirect!

    # This adds 'signals' to session[:bot_config]
    assert_response :ok
  end
end
