require 'test_helper'

# B3. The residual half of the divergence, on a screen the original report never named: an existing
# bot re-picking an asset. attach_exchanges knew nothing about the bot's venue, so it advertised
# every exchange that listed the asset — including ones this bot can never trade on, because a
# started bot's exchange is fixed.
class Bots::AssetSearchesControllerTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true)
    @user = create(:user, setup_completed: true)
    sign_in @user

    @binance = create(:binance_exchange)
    @kraken = create(:kraken_exchange)
    @bot = create(:dca_single_asset, user: @user, exchange: @binance)
  end

  test 're-picking an asset on an existing bot offers only its own venue' do
    eth = create(:asset, symbol: 'ETH', name: 'Ethereum', external_id: 'ethereum-b3')
    create(:ticker, exchange: @binance, base_asset: eth, quote_asset: @bot.quote_asset)
    create(:ticker, exchange: @kraken, base_asset: eth, quote_asset: @bot.quote_asset)

    get edit_bot_asset_search_path(bot_id: @bot.id, asset_field: 'base_asset_id')

    assert_response :ok
    assert_match 'ETH', response.body
    assert_match 'title="Binance"', response.body
    assert_no_match 'title="Kraken"', response.body,
                    'the bot trades on Binance; Kraken is not an option for it'
  end
end
