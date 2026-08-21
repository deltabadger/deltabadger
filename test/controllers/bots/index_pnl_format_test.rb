# frozen_string_literal: true

require 'test_helper'

# The header total and every bot tile ship BOTH formats in the markup — percent and USD amount —
# and one class on <html>, flipped by clicking the header, decides which one is visible.
#
# Rendering both is what makes the choice survive: the header and each tile are replaced by
# independent turbo broadcasts, so anything stored per element (a `d-none` toggled in place)
# is lost the moment a bot's PnL job lands.
class Bots::IndexPnlFormatTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true) # satisfies the onboarding gate (an admin must exist)
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    shared = { user: @user, exchange: @exchange, base_asset: @btc, quote_asset: create(:asset, :usd) }

    @bot = create(:dca_single_asset, **shared)
    create(:dca_single_asset, **shared) # a second bot: a lone bot redirects to its own page

    stub_metrics(pnl: 0.25, invested: 100, value: 125)
    User.any_instance.stubs(:global_pnl_snapshot).returns(
      { result: { percent: 0.25.to_d, profit_usd: 25.to_d }, loading: false }
    )

    sign_in @user
  end

  def stub_metrics(pnl:, invested:, value:)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache).returns(
      { pnl: pnl.to_d, total_quote_amount_invested: invested.to_d, total_amount_value_in_quote: value.to_d }
    )
  end

  def tile_pnl
    "##{@bot.dom_id(@bot, :pnl)}"
  end

  test 'the header total carries both the percent and the USD amount' do
    get bots_path

    assert_select '#global-pnl .pnl-percent', text: /\+25\.00%/
    assert_select '#global-pnl .pnl-amount', text: /\+\$25/
  end

  test 'a bot tile carries both the percent and the profit in USD' do
    get bots_path

    assert_select "#{tile_pnl} .pnl-percent", text: /25\.00%/
    assert_select "#{tile_pnl} .pnl-amount", text: /\+\$25\.00/
  end

  test 'a losing tile shows the amount with its minus sign' do
    stub_metrics(pnl: -0.1, invested: 100, value: 90)

    get bots_path

    assert_select "#{tile_pnl} .pnl-amount", text: /-\$10\.00/
  end

  # A EUR bot next to a USDT one must not read as two different scales — the tile amount is
  # the same currency as the account total, whatever the bot trades in.
  test 'a non-USD bot has its profit converted with the cached rate' do
    eur_bot = create(:dca_single_asset, user: @user, exchange: @exchange, base_asset: @btc,
                                        quote_asset: create(:asset, :eur))
    Utilities::Currency.stubs(:exchange_rate).with(from: 'USD', to: 'USD', cache_only: true)
                       .returns(Result::Success.new(1.0))
    Utilities::Currency.stubs(:exchange_rate).with(from: 'EUR', to: 'USD', cache_only: true)
                       .returns(Result::Success.new(1.1))

    get bots_path

    assert_select "##{eur_bot.dom_id(eur_bot, :pnl)} .pnl-amount", text: /\+\$27\.50/
  end

  # The index never makes a live FX call, so an uncached rate means no amount — the account
  # total is in its spinner in exactly that case.
  test 'a bot whose rate is not cached renders the percent alone' do
    eur_bot = create(:dca_single_asset, user: @user, exchange: @exchange, base_asset: @btc,
                                        quote_asset: create(:asset, :eur))

    get bots_path

    assert_select "##{eur_bot.dom_id(eur_bot, :pnl)} .pnl-percent", text: /25\.00%/
    assert_select "##{eur_bot.dom_id(eur_bot, :pnl)} .pnl-amount", false
  end

  # ...and it asks for the refresh that fills the amount in, rather than staying blank until
  # the next page load: the job runs outside the request, so it may fetch the rate.
  test 'a bot whose rate is not cached is queued for an async refresh' do
    eur_bot = create(:dca_single_asset, user: @user, exchange: @exchange, base_asset: @btc,
                                        quote_asset: create(:asset, :eur))

    get bots_path

    args = css_select('.itiles').first['data-broadcast--on-connect-method-args-value']
    assert_includes JSON.parse(args)['bot_ids'], eur_bot.id
  end

  # $0 is $0 in every currency, so break-even must not wait on a rate it doesn't need.
  test 'a break-even bot shows its zero without any rate' do
    eur_bot = create(:dca_single_asset, user: @user, exchange: @exchange, base_asset: @btc,
                                        quote_asset: create(:asset, :eur))
    stub_metrics(pnl: 0, invested: 100, value: 100)

    get bots_path

    assert_select "##{eur_bot.dom_id(eur_bot, :pnl)} .pnl-amount", text: /\$0\.00/
  end

  test 'a tile with no metrics yet renders neither format' do
    Bots::DcaSingleAsset.any_instance.stubs(:metrics_with_current_prices_from_cache).returns(nil)

    get bots_path

    assert_select "#{tile_pnl} .pnl-percent", false
    assert_select "#{tile_pnl} .pnl-amount", false
  end

  # Solid Cache reads are queries, and this page renders eighty tiles: the rate for a currency
  # is looked up once for the whole page, not once per bot holding it.
  test 'the tile amounts cost one FX lookup per currency, not per bot' do
    Utilities::Currency.expects(:exchange_rate).with(from: 'USD', to: 'USD', cache_only: true)
                       .once.returns(Result::Success.new(1.0))

    get bots_path

    assert_response :ok
  end

  # The account's fiat denominator is a display step, not a different sum: everything stays
  # computed in USD and one rate turns it into the chosen currency for the whole page.
  test 'the header total and the tiles read in the account currency' do
    @user.update!(display_currency: 'PLN')
    Utilities::Currency.stubs(:exchange_rate).with(from: 'USD', to: 'USD', cache_only: true)
                       .returns(Result::Success.new(1.0))
    Utilities::Currency.stubs(:exchange_rate).with(from: 'USD', to: 'PLN')
                       .returns(Result::Success.new(4.0))

    get bots_path

    assert_select '#global-pnl .pnl-amount', text: /\+100 zł/
    assert_select "#{tile_pnl} .pnl-amount", text: /\+100\.00 zł/
  end

  # One rate for the page, whatever it is: the tiles are already careful about this and the
  # denominator must not undo it.
  test 'the chosen currency costs one FX lookup for the whole page' do
    @user.update!(display_currency: 'PLN')
    Utilities::Currency.stubs(:exchange_rate).with(from: 'USD', to: 'USD', cache_only: true)
                       .returns(Result::Success.new(1.0))
    Utilities::Currency.expects(:exchange_rate).with(from: 'USD', to: 'PLN')
                       .once.returns(Result::Success.new(4.0))

    get bots_path

    assert_response :ok
  end
end
