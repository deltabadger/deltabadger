# frozen_string_literal: true

require 'test_helper'

class Bots::DcaMultiAssetsShowSettingsTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @assets = %w[AAA BBB CCC].map do |symbol|
      create(:asset, symbol:, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}", color: '#4477AA')
    end
    @bot = create(:dca_multi_asset, user: @user, base_assets: @assets,
                                    allocations: { @assets[0] => 0.5, @assets[1] => 0.3, @assets[2] => 0.2 })
  end

  test 'renders one range slider with a thumb per member, in allocation order' do
    get bot_path(id: @bot.id)

    assert_response :success
    @assets.each do |asset|
      assert_select 'input[type="range"][name=?][min="0"][max="100"][step="0.1"]',
                    "bots_dca_multi_asset[allocations][#{asset.id}]", count: 1
    end
    assert_select 'input.allocation__input[type="number"][step="0.1"]', count: @assets.size
    # Shift coarsens a 0-100 slider to whole percent, and only the body's controller listens for it.
    assert_select 'body[data-controller~="coarse-slider"]'
    assert_select '.slider__style__thumb', count: @assets.size
    names = css_select('[data-bot--allocation-target="row"] input[type="range"]').map { |input| input['name'] }
    assert_equal @assets.map { |asset| "bots_dca_multi_asset[allocations][#{asset.id}]" }, names
  end

  test 'renders the total, and reveals Normalize with a hint only when unbalanced' do
    get bot_path(id: @bot.id)

    assert_select '[data-bot--allocation-target="total"]', text: '100.0%'
    assert_select 'button.asset-allocations__normalize[hidden]', count: 1

    settings = @bot.settings.merge(
      'allocations' => {
        @assets[0].id.to_s => 0.5,
        @assets[1].id.to_s => 0.25,
        @assets[2].id.to_s => 0.1
      }
    )
    @bot.update_columns(settings:)
    get bot_path(id: @bot.id)

    assert_select '[data-bot--allocation-target="total"]', text: '85.0%'
    assert_select 'button.asset-allocations__normalize:not([hidden])', count: 1
    # The blocker reads once, next to the Start button it disables — the status bar, not the button.
    assert_select "##{dom_id(@bot, :status_bar)}", text: /#{I18n.t('bot.dca_multi_asset.normalize_first')}/
    assert_select "##{dom_id(@bot, :status_button)} .status-button__hint", count: 0
    assert_select "##{dom_id(@bot, :status_button)} button[disabled]", count: 1
  end

  test 'sliders carry no redistribution targets' do
    get bot_path(id: @bot.id)

    assert_select '[data-bot--allocation-target="input"]', count: @assets.size
    assert_select '[data-bot--allocation-target="total"]', count: 1
    assert_select '[data-bot--allocation-target="remainder"]', count: 0
  end

  test 'sliders are disabled and add/remove hidden while the bot works' do
    @bot.update_columns(status: Bot.statuses[:waiting])

    get bot_path(id: @bot.id)

    assert_select '.asset-allocation input[type="range"][disabled]', count: @assets.size
    assert_select '.asset-allocation__add', count: 0
    assert_select '.asset-allocation__remove', count: 0
  end

  test 'a stopped bot with a pending rebalance shows its composition locked' do
    @bot.update_columns(status: Bot.statuses[:stopped])
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 10)

    get bot_path(id: @bot.id)

    assert_select '.asset-allocation input[type="range"][disabled]', count: @assets.size
    assert_select '.asset-allocation__add', count: 0
    assert_select '.asset-allocation__remove', count: 0
    assert_select '.asset-allocations__normalize', count: 0
    assert_select '#exchange_select .dropdown__item--note',
                  text: I18n.t('bot.exchange_menu.locked_while_rebalancing')
  end

  test 'the add link opens the asset search for add_asset_id' do
    get bot_path(id: @bot.id)

    assert_select 'a.asset-allocation__add[href=?][data-turbo-frame="modal"]',
                  edit_bot_asset_search_path(bot_id: @bot.id, asset_field: :add_asset_id), count: 1
  end

  test 'the remove control is a submit button named remove_asset_id inside the settings form' do
    get bot_path(id: @bot.id)

    assert_select 'form[action=?] button.asset-allocation__remove[name=?]',
                  bot_path(id: @bot.id), 'bots_dca_multi_asset[remove_asset_id]', count: @assets.size
  end

  test 'the asset search modal results patch add_asset_id' do
    candidate = create(:asset, symbol: 'DDD', name: 'Coin DDD', external_id: 'coin-ddd')
    create(:ticker, exchange: @bot.exchange, base_asset: candidate, quote_asset: @bot.quote_asset)

    get edit_bot_asset_search_path(bot_id: @bot.id, asset_field: :add_asset_id)

    assert_response :success
    assert_select 'form[action=?] input[name="_method"][value="patch"]', bot_path(id: @bot.id), count: 1
    assert_select 'button[name=?][value=?]', 'bots_dca_multi_asset[add_asset_id]', candidate.id.to_s, count: 1
  end

  test 'at MAX_ASSETS the add link is gone' do
    assets = Array.new(Bots::DcaMultiAsset::MAX_ASSETS) do |index|
      create(:asset, symbol: "M#{index}", name: "Member #{index}", external_id: "member-#{index}")
    end
    bot = create(:dca_multi_asset, user: @user, exchange: @bot.exchange, quote_asset: @bot.quote_asset,
                                   base_assets: assets, with_api_key: false)

    get bot_path(id: bot.id)

    assert_select '.asset-allocation', count: Bots::DcaMultiAsset::MAX_ASSETS
    assert_select '.asset-allocation__add', count: 0
  end

  test 'no nested form and no formmethod' do
    get bot_path(id: @bot.id)

    assert_select '#settings form form', count: 0
    assert_select '#settings [formmethod]', count: 0
  end
end
