require 'test_helper'

# The prompt is the only place the redeployable cash is ever shown, so a partial that fails to render
# — or one that renders when the answer would be refused — is the whole feature broken with nothing
# else to notice it.
class Bots::RedeployPromptRenderTest < ActionDispatch::IntegrationTest
  def setup
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @bot = create(:dca_index, user: @user)
    asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
    create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
    sign_in @user
  end

  test 'the prompt names the amount on offer' do
    liquidated(150)

    get bot_path(id: @bot.id)

    assert_response :success
    assert_match(/Redeploy/i, response.body)
    assert_match(/150/, response.body)
  end

  test 'no prompt when there is nothing to redeploy' do
    get bot_path(id: @bot.id)

    assert_response :success
    assert_no_match(/Redeploy/i, response.body)
  end

  test 'no prompt once the offer has been declined' do
    liquidated(150)
    @bot.decline_redeploy!

    get bot_path(id: @bot.id)

    assert_response :success
    assert_no_match(/Redeploy/i, response.body)
  end

  # Offering an action the job will refuse is worse than offering nothing.
  test 'no prompt while a rebalance is mid-swap' do
    liquidated(150)
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING)

    get bot_path(id: @bot.id)

    assert_response :success
    assert_no_match(/Redeploy/i, response.body)
  end

  test 'a halted batch shows the halt and a way out of it, not the prompt' do
    liquidated(150)
    @bot.start_redeploy_placement!
    @bot.flag_redeploy_ambiguous!

    get bot_path(id: @bot.id)

    assert_response :success
    assert_match(/unconfirmed/i, response.body)
    assert_match(/redeploy_resolutions/, response.body, 'the Clear button has to be reachable')
  end

  test 'the amount is hidden when the user hides balances' do
    liquidated(150)
    @user.update!(hide_balances: true)

    get bot_path(id: @bot.id)

    assert_response :success
    assert_no_match(/150/, response.body)
  end

  # The composition table used to be "everything that is not a quitter". Those sets diverged once a
  # holding too small to sell stopped counting as a quitter — it then fell through into this table
  # and was rendered as a constituent of the index, at 0.00.
  test 'dust too small to sell is shown in neither table' do
    member = create(:asset, symbol: 'MEM', name: 'Member', external_id: 'coin-mem')
    member_ticker = create(:ticker, exchange: @bot.exchange, base_asset: member,
                                    quote_asset: @bot.quote_asset, minimum_base_size: 1)
    BotIndexAsset.create!(bot: @bot, asset: member, ticker: member_ticker,
                          target_allocation: 1.0, in_index: true, entered_at: Time.current)
    dust = create(:asset, symbol: 'DUST', name: 'Dust', external_id: 'coin-dust')
    create(:ticker, exchange: @bot.exchange, base_asset: dust, quote_asset: @bot.quote_asset,
                    minimum_base_size: 1)
    @bot.stubs(:metrics_with_current_prices).returns(
      asset_values: { 'MEM' => { amount: 10.to_d, current_value: 100.to_d, quote_invested: 90.to_d,
                                 avg_price: 9.to_d, pnl_percentage: 0.1 },
                      'DUST' => { amount: 0.004.to_d, current_value: 0.to_d, quote_invested: 1.to_d,
                                  avg_price: 1.to_d, pnl_percentage: 0 } },
      prices_stale: false, asset_breakdown: {}, chart: { labels: [] }
    )

    get bot_path(id: @bot.id)

    assert_response :success
    assert_match(/MEM/, response.body, 'the member belongs in the composition table')
    assert_no_match(/DUST/, response.body, 'dust is neither a member nor a sellable quitter')
  end

  # The note used to be small print under the figure, competing with it.
  test 'the realised P/L note is behind an icon, not printed under the figure' do
    # A gain, or the cell is not rendered at all: Realised P/L only appears once there is one.
    bought(100)
    liquidated(150)

    get bot_path(id: @bot.id)

    assert_response :success
    # Sibling of the label, inside the controller's element — not nested in the icon, where the
    # widget's overflow:hidden clipped it away.
    assert_select '.data-grid__item[data-controller=tooltip] > .tooltip',
                  text: /Rebalancing is not counted/
    assert_select '.data-grid__item .label--info .tooltip-info-icon', 1
    assert_select '.data-grid__item small.label', false, 'no longer a line of small print'
  end

  private

  def bought(quote)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :closed, external_id: "b-#{SecureRandom.hex(4)}",
                         side: :buy, transaction_type: 'REGULAR', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: 100, amount: quote.to_d / 100,
                         amount_exec: quote.to_d / 100, quote_amount: quote, quote_amount_exec: quote)
  end

  def liquidated(quote)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :closed, external_id: "l-#{SecureRandom.hex(4)}",
                         side: :sell, transaction_type: 'LIQUIDATION', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: 100, amount: quote.to_d / 100,
                         amount_exec: quote.to_d / 100, quote_amount: quote, quote_amount_exec: quote)
  end
end
