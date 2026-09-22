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

  # == one asset: it reads like the pair bot it replaces ==

  test 'a one-asset basket says what it buys: Invest 100 USD / day into AAA' do
    get bot_path(id: one_asset_bot.id)

    assert_select '.main-rule .conversational', text: /#{I18n.t('bot.conversational.into')}/
    assert_select '.main-rule .conversational .ticker', text: 'AAA'
  end

  test 'a selling one-asset basket names the asset it sells' do
    bot = one_asset_bot
    bot.set_missed_quote_amount
    bot.update!(direction: 'selling')

    get bot_path(id: bot.id)

    assert_select '.main-rule .conversational .ticker', text: 'AAA'
  end

  test 'a one-asset basket has no weight controls' do
    get bot_path(id: one_asset_bot.id)

    assert_select '.asset-allocation', count: 0
    assert_select '[data-bot--allocation-target="total"]', count: 0
    assert_select '.asset-allocations__normalize', count: 0
    assert_select '.asset-allocation__remove', count: 0
    assert_select 'form.main-rule[data-controller~="bot--allocation"]', count: 0
  end

  test 'a one-asset basket can still grow into a basket' do
    get bot_path(id: one_asset_bot.id)

    assert_select 'a.asset-allocation__add', count: 1
  end

  test 'a one-asset basket offers neither the market-cap nor the rebalance rule' do
    # One asset weighs 100% by any rule, and has nothing to drift from.
    @assets[0].update!(market_cap: 750.0)

    get bot_path(id: one_asset_bot.id)

    assert_select "input[name='bots_dca_multi_asset[weighting]']", count: 0
    assert_select "[name='bots_dca_multi_asset[rebalance_enabled]']", count: 0
  end

  test 'a selling one-asset basket offers the base cap; a selling two-asset basket does not' do
    bot = one_asset_bot
    bot.set_missed_quote_amount
    bot.update!(direction: 'selling', sell_denomination: 'base')
    get bot_path(id: bot.id)
    assert_select "[name='bots_dca_multi_asset[base_amount_limit]']", count: 1

    @bot.set_missed_quote_amount
    @bot.update!(direction: 'selling')
    get bot_path(id: @bot.id)
    assert_select "[name='bots_dca_multi_asset[base_amount_limit]']", count: 0
  end

  test "a two-asset basket's sentence names no asset" do
    get bot_path(id: @bot.id)

    assert_select '.main-rule .conversational', text: /#{I18n.t('bot.conversational.into')}/, count: 0
  end

  test 'a two-asset basket offers remove on both members' do
    two = create(:dca_multi_asset, user: @user, exchange: @bot.exchange, quote_asset: @bot.quote_asset,
                                   base_assets: @assets.first(2), allocations: { @assets[0] => 0.5, @assets[1] => 0.5 })

    get bot_path(id: two.id)

    assert_select 'button.asset-allocation__remove', count: 2
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

  test 'over the cap the status bar says how many to remove and Start stays disabled' do
    assets = Array.new(Bots::DcaMultiAsset::MAX_ASSETS + 3) do |index|
      create(:asset, symbol: "M#{index}", name: "Member #{index}", external_id: "member-#{index}")
    end
    bot = create(:dca_multi_asset, user: @user, exchange: @bot.exchange, quote_asset: @bot.quote_asset,
                                   base_assets: assets, status: :stopped)

    get bot_path(id: bot.id)

    assert_response :success
    assert_select "##{dom_id(bot, :status_bar)} .bot-control__status__text",
                  text: I18n.t('bot.dca_multi_asset.too_many_assets', count: 3)
    assert_select "##{dom_id(bot, :status_button)} .status-button__hint", count: 0
    assert_select "##{dom_id(bot, :status_button)} button[disabled]", count: 1
    assert_select '.asset-allocation__add', count: 0
    assert_select 'button.asset-allocation__remove', count: assets.size

    # Back under the cap the weights no longer add up, and that is the next thing to say.
    bot.update_columns(settings: bot.settings.merge('allocations' => bot.allocations.except(*assets.first(3).map { |a| a.id.to_s })))
    get bot_path(id: bot.id)

    assert_select "##{dom_id(bot, :status_bar)} .bot-control__status__text",
                  text: I18n.t('bot.dca_multi_asset.normalize_first')
    assert_select "##{dom_id(bot, :status_button)} button[disabled]", count: 1
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

  private

  # The single-asset bot's page rendered with its pair delisted; the one-asset basket that replaces it must
  # too. Its tradeable tickers are empty then, so precision comes from the membership's ticker.
  test 'a one-asset basket whose pair was delisted renders its page and its start question' do
    bot = one_asset_bot
    bot.composition_tickers.sole.update!(available: false)
    bot.set_missed_quote_amount
    bot.update!(quote_amount_limited: true, quote_amount_limit: 500)
    bot.update_columns(status: Bot.statuses[:stopped])

    get bot_path(id: bot.id)
    assert_response :success
    assert_select '#settings-amount-limit-info'

    get edit_bot_start_path(bot_id: bot.id)
    assert_response :success
  end

  def one_asset_bot
    @one_asset_bot ||= create(:dca_multi_asset, user: @user, exchange: @bot.exchange, quote_asset: @bot.quote_asset,
                                                base_assets: [@assets[0]])
  end
end
