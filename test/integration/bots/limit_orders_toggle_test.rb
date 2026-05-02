require 'test_helper'

class Bots::LimitOrdersToggleTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    @bitcoin = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    sign_in @user
  end

  test 'Hyperliquid bot renders limit_ordered checkbox checked and disabled' do
    exchange = create(:hyperliquid_exchange)
    create(:api_key, user: @user, exchange: exchange, key_type: :trading, status: :correct,
                     raw_key: "0x#{'a' * 40}", raw_secret: 'b' * 64)
    bot = create(:dca_single_asset,
                 user: @user, exchange: exchange,
                 base_asset: @bitcoin, quote_asset: @usd,
                 status: :stopped, with_api_key: false)

    get bot_path(id: bot.id)
    assert_response :ok
    assert_select 'input[type=checkbox][name="bots_dca_single_asset[limit_ordered]"][checked][disabled]'
    assert_select '.tooltip', text: 'Hyperliquid does not support market orders on spot.'
  end

  test 'non-Hyperliquid stopped bot renders limit_ordered checkbox enabled' do
    exchange = create(:binance_exchange)
    create(:api_key, user: @user, exchange: exchange, key_type: :trading, status: :correct)
    bot = create(:dca_single_asset,
                 user: @user, exchange: exchange,
                 base_asset: @bitcoin, quote_asset: @usd,
                 status: :stopped, with_api_key: false)

    get bot_path(id: bot.id)
    assert_response :ok
    assert_select 'input[type=checkbox][name="bots_dca_single_asset[limit_ordered]"]:not([disabled])'
  end
end
