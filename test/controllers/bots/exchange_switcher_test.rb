require 'test_helper'

# The exchange switcher was gated on `stopped?`, so a bot that had never been started — the exact
# state a bot sits in right after the wizard, and the state a user lands in with a rejected key —
# got no dropdown at all. Not-working is the gate: `created` and `stopped` both switch freely.
class ExchangeSwitcherTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user

    @base = create(:asset, :bitcoin)
    @quote = create(:asset, :usd)
    @kraken = create(:kraken_exchange)
    create(:ticker, exchange: @kraken, base_asset: @base, quote_asset: @quote)
  end

  def bot_with(status:, with_api_key:)
    create(:dca_single_asset, status, user: @user, base_asset: @base, quote_asset: @quote,
                                      with_api_key:)
  end

  test 'a never-started bot with a disconnected key can still be switched to another exchange' do
    bot = create(:dca_single_asset, user: @user, base_asset: @base, quote_asset: @quote,
                                    with_api_key: false)

    get bot_path(id: bot.id)

    assert_response :success
    assert_match 'dropdown--exchanges', response.body
    assert_match @kraken.name, response.body
  end

  # Still not switchable, but no longer silent about it: rendering nothing left the chip taking
  # the click and showing an empty page tint, which reads as something else blocking the control.
  test 'a working bot is told to stop rather than handed a menu that never opens' do
    bot = bot_with(status: :started, with_api_key: true)

    get bot_path(id: bot.id)

    assert_response :success
    assert_match I18n.t('bot.exchange_menu.locked_while_running'), response.body
    refute_match '[exchange_id]', response.body
  end

  test 'a stopped multi-asset bot can switch to another venue that carries the basket' do
    ethereum = create(:asset, :ethereum)
    binance = create(:binance_exchange)
    create(:ticker, exchange: binance, base_asset: @base, quote_asset: @quote)
    create(:ticker, exchange: binance, base_asset: ethereum, quote_asset: @quote)
    create(:ticker, exchange: @kraken, base_asset: ethereum, quote_asset: @quote)
    bot = create(:dca_multi_asset, :stopped, user: @user, exchange: binance,
                                             base_assets: [@base, ethereum], quote_asset: @quote)

    get bot_path(id: bot.id)

    assert_response :success
    assert_select "#exchange_select button[value='#{@kraken.id}']", count: 1
  end
end
