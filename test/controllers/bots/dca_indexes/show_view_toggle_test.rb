require 'test_helper'

# Renders the index bot show page and asserts the pie/list view toggle scaffolding
# is present: the two toggle buttons, the list/pie/pieSvg Stimulus targets, the
# per-row hex `data-color` the donut needs, and the bot-id value used for the
# per-bot localStorage preference.
#
# The `.index-assets` preview block only renders when `current_index_preview`
# returns rows, which needs matching exchange tickers plus a stubbed
# `MarketData.get_top_coins` — the `:dca_index` factory alone renders nothing.
class Bots::DcaIndexes::ShowViewToggleTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user

    @exchange = create(:kraken_exchange)
    @quote = create(:asset, :eur)

    create_candidate('coin-a', 'AAA')
    create_candidate('coin-b', 'BBB')
    create_candidate('coin-c', 'CCC')
    MarketData.stubs(:get_top_coins).returns(
      Result::Success.new(%w[coin-a coin-b coin-c].each_with_index.map do |id, i|
        { 'id' => id, 'market_cap' => (100 - i).to_f, 'current_price' => 1.0 }
      end)
    )

    @bot = create(:dca_index, user: @user, exchange: @exchange, quote_asset: @quote)
  end

  test 'renders the allocation preview rows with a hex data-color' do
    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '.index-assets .index-asset[data-color^="#"]', minimum: 1
  end

  test 'both the pie and the list are themselves the toggle (no dedicated buttons)' do
    get bot_path(id: @bot.id)

    assert_response :success
    # Clicking either visualization toggles the view — no separate toggle buttons.
    assert_select '[data-index-allocation-target~="pie"][data-action*="index-allocation#toggle"]'
    assert_select '.index-assets[data-action*="index-allocation#toggle"]'
    assert_select 'button[data-action*="index-allocation#showPie"]', false
  end

  test 'renders the list, pie and pieSvg Stimulus targets' do
    get bot_path(id: @bot.id)

    assert_response :success
    # The existing assets list keeps its `assets` target and also gains `list`.
    assert_select '.index-assets[data-index-allocation-target~="assets"][data-index-allocation-target~="list"]'
    assert_select '[data-index-allocation-target~="pie"]'
    assert_select '[data-index-allocation-target~="pieSvg"]'
  end

  test 'exposes the bot id for the per-bot localStorage preference' do
    get bot_path(id: @bot.id)

    assert_response :success
    assert_select "[data-index-allocation-bot-id-value=\"#{@bot.id}\"]"
  end

  private

  def create_candidate(external_id, symbol)
    asset = create(:asset, external_id: external_id, symbol: symbol)
    create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @quote)
  end
end
