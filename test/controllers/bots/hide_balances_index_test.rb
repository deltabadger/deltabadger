# frozen_string_literal: true

require 'test_helper'

# With balances hidden the dashboard reads in percent only. The fiat half of every PnL is not
# rendered at all — not hidden with CSS — so the header stops being a switch and the tiles stop
# costing an FX conversion they have nothing to show.
class Bots::HideBalancesIndexTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true) # satisfies the onboarding gate (an admin must exist)
    @user = create(:user, hide_balances: true)
    @exchange = create(:binance_exchange)
    shared = { user: @user, exchange: @exchange, base_asset: create(:asset, :bitcoin),
               quote_asset: create(:asset, :usd) }

    @bot = create(:dca_single_asset, **shared)
    create(:dca_single_asset, **shared) # a second bot: a lone bot redirects to its own page

    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache).returns(
      { pnl: 0.25.to_d, total_quote_amount_invested: 100.to_d, total_amount_value_in_quote: 125.to_d }
    )
    User.any_instance.stubs(:global_pnl_snapshot).returns(
      { result: { percent: 0.25.to_d, profit_usd: 25.to_d }, loading: false }
    )

    sign_in @user
  end

  def tile_pnl
    "##{@bot.dom_id(@bot, :pnl)}"
  end

  test 'the header total reads in percent and states no amount' do
    get bots_path

    assert_select '#global-pnl .pnl-percent', text: /\+25\.00%/
    assert_select '#global-pnl .pnl-amount', false
  end

  test 'the header is no longer a switch' do
    get bots_path

    assert_select '#global-pnl [data-controller~=pnl-format]', false
  end

  test 'a bot tile reads in percent and states no amount' do
    get bots_path

    assert_select "#{tile_pnl} .pnl-percent", text: /25\.00%/
    assert_select "#{tile_pnl} .pnl-amount", false
  end

  # The amount was the only reason the index converted anything. Hiding it should cost less, not
  # the same.
  test 'the tiles ask for no exchange rate at all' do
    Utilities::Currency.expects(:exchange_rate).never

    get bots_path

    assert_response :ok
  end

  # ...and having nothing to fill in, no tile asks for the async refresh that would fill it. A bot
  # in a currency with no cached rate is exactly the tile that would ask for one.
  test 'no tile is queued for a refresh it has nothing to gain from' do
    create(:dca_single_asset, user: @user, exchange: @exchange,
                              base_asset: Asset.find_by(symbol: 'BTC'),
                              quote_asset: create(:asset, :eur))

    get bots_path

    assert_nil css_select('.itiles').first['data-broadcast--on-connect-method-args-value']
  end

  # The tile is replaced by a background job with no request context, so it has to read the
  # preference off the bot's user.
  test 'a broadcast tile states no amount either' do
    @bot.stubs(:metrics_with_current_prices).returns(
      { pnl: 0.25.to_d, total_quote_amount_invested: 100.to_d, total_amount_value_in_quote: 125.to_d }
    )
    @bot.expects(:profit_in_usd).never

    @bot.broadcast_pnl_update
  end
end
