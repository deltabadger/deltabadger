# frozen_string_literal: true

require 'test_helper'

class Bots::DcaMultiAssetsSettingsUpdateTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @bot = create(:dca_multi_asset, user: @user)
    @first, @second = @bot.base_assets
  end

  test 'PATCH allocations is saved as posted; an off-100 total leaves the bot unstartable' do
    patch_allocations(@first.id => '70', @second.id => '70')

    assert_response :success
    assert_equal({ @first.id.to_s => 0.7, @second.id.to_s => 0.7 }, @bot.reload.allocations)
    assert_equal({ @first.id => 0.5, @second.id => 0.5 }, composition_weights)
    assert_not @bot.valid?(:start)
    assert_match I18n.t('bot.dca_multi_asset.normalize_first'), response.body
    assert_match(/disabled="disabled"/, response.body)
  end

  test 'a market-cap basket renders the derived weights, and its sliders are read-only' do
    @first.update!(market_cap: 750.0)
    @second.update!(market_cap: 250.0)

    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { weighting: 'market_cap' } },
                                 as: :turbo_stream

    assert_response :success
    assert_equal({ @first.id => 0.75, @second.id => 0.25 }, composition_weights)
    # The header total must come from the same source as the rows, or it prints the slider sum.
    assert_match(/100\.0%/, response.body)
    assert_match(/disabled/, response.body)
  end

  test 'the market-cap rule is offered when every member carries a market cap' do
    @first.update!(market_cap: 750.0)
    @second.update!(market_cap: 250.0)

    get bot_path(id: @bot.id)

    assert_select "input[name='bots_dca_multi_asset[weighting]']"
  end

  test 'the rule is not offered on a basket of stocks, which carry none' do
    # The case that prompted this: on a QQQM/IBIT basket the control appeared and did nothing.
    @first.update!(market_cap: nil, category: 'Stock')
    @second.update!(market_cap: nil, category: 'Stock')

    get bot_path(id: @bot.id)

    assert_select "input[name='bots_dca_multi_asset[weighting]']", false
  end

  test 'the rule reads as a toggle, not as a picker' do
    @first.update!(market_cap: 750.0)
    @second.update!(market_cap: 250.0)

    get bot_path(id: @bot.id)

    assert_select "input[type='checkbox'][name='bots_dca_multi_asset[weighting]'][value='market_cap']"
    assert_select "select[name='bots_dca_multi_asset[weighting]']", false
  end

  test 'unchecking the rule puts the basket back on its own weights' do
    @first.update!(market_cap: 750.0)
    @second.update!(market_cap: 250.0)
    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { weighting: 'market_cap' } },
                                 as: :turbo_stream

    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { weighting: 'manual' } },
                                 as: :turbo_stream

    assert_not @bot.reload.market_cap_weighted?
    assert_equal({ @first.id => 0.5, @second.id => 0.5 }, composition_weights)
  end

  test 'PATCH add_asset_id admits a third asset at zero' do
    third = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
    create(:ticker, exchange: @bot.exchange, base_asset: third, quote_asset: @bot.quote_asset)

    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { add_asset_id: third.id } }, as: :turbo_stream

    assert_response :success
    assert_equal [@first.id, @second.id, third.id], @bot.reload.base_asset_ids
    assert_equal [0.0, 0.5, 0.5], @bot.allocations.values.sort
    assert_equal 3, @bot.bot_index_assets.in_index.count
  end

  test 'PATCH add_asset_id of an asset the exchange does not trade is refused' do
    unsupported = create(:asset, symbol: 'NOPE', name: 'Unsupported Asset', external_id: 'unsupported')

    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { add_asset_id: unsupported.id } }, as: :turbo_stream

    assert_response :unprocessable_entity
    assert_includes flash[:alert], unsupported.symbol
    assert_equal [@first.id, @second.id], @bot.reload.base_asset_ids
  end

  test 'PATCH remove_asset_id below two assets is refused with 422' do
    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { remove_asset_id: @second.id } }, as: :turbo_stream

    assert_response :unprocessable_entity
    assert_equal [@first.id, @second.id], @bot.reload.base_asset_ids
  end

  test 'PATCH normalize_allocations squeezes to 100' do
    patch bot_path(id: @bot.id), params: {
      bots_dca_multi_asset: {
        allocations: { @first.id => '30', @second.id => '30' },
        normalize_allocations: '1'
      }
    }, as: :turbo_stream

    assert_response :success
    assert_equal({ @first.id.to_s => 0.5, @second.id.to_s => 0.5 }, @bot.reload.allocations)
  end

  test 'PATCH remove_asset_id leaves the other weights alone' do
    third = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
    create(:ticker, exchange: @bot.exchange, base_asset: third, quote_asset: @bot.quote_asset)
    @bot.set_missed_quote_amount
    @bot.update!(
      allocations: {
        @first.id.to_s => 0.5,
        @second.id.to_s => 0.25,
        third.id.to_s => 0.25
      }
    )

    patch bot_path(id: @bot.id), params: {
      bots_dca_multi_asset: { remove_asset_id: @first.id }
    }, as: :turbo_stream

    assert_response :success
    assert_equal({ @second.id.to_s => 0.25, third.id.to_s => 0.25 }, @bot.reload.allocations)
  end

  test 'PATCH on a working bot refuses composition changes and allows quote_amount' do
    @bot.update_columns(status: Bot.statuses[:waiting])

    patch_allocations(@first.id => '60', @second.id => '40')
    assert_response :unprocessable_entity
    assert_equal({ @first.id.to_s => 0.5, @second.id.to_s => 0.5 }, @bot.reload.allocations)

    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { quote_amount: '125' } }, as: :turbo_stream
    assert_response :success
    assert_equal 125.0, @bot.reload.quote_amount
  end

  test 'the show page renders the multi-asset settings and metrics partials' do
    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '#settings'
    assert_select "#settings form[action='#{bot_path(id: @bot.id)}']"
    assert_select '#metrics'
  end

  test 'the tile names the first three symbols' do
    sol = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
    ada = create(:asset, symbol: 'ADA', name: 'Cardano', external_id: 'cardano')
    [sol, ada].each do |asset|
      create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
    end
    @bot.set_missed_quote_amount
    @bot.update!(allocations: @bot.equal_allocations([@first.id, @second.id, sol.id, ada.id]))
    create(:dca_single_asset, user: @user, exchange: @bot.exchange,
                              base_asset: @first, quote_asset: @bot.quote_asset)

    get bots_path

    assert_response :success
    assert_select '.bot-tile__title', /BTC, ETH, SOL \+1/
  end

  private

  def patch_allocations(allocations)
    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { allocations: allocations } }, as: :turbo_stream
  end

  def composition_weights
    @bot.bot_index_assets.in_index.pluck(:asset_id, :target_allocation).to_h
  end
end
