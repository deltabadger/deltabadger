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

  private

  def liquidated(quote)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :closed, external_id: "l-#{SecureRandom.hex(4)}",
                         side: :sell, transaction_type: 'LIQUIDATION', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: 100, amount: quote.to_d / 100,
                         amount_exec: quote.to_d / 100, quote_amount: quote, quote_amount_exec: quote)
  end
end
