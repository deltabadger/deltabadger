# frozen_string_literal: true

require 'test_helper'

# The dashboard's select mode is client-side, so this covers the markup the Stimulus controller
# hangs off: the Merge button, the bar and its form, and what each tile says about itself.
class Bots::IndexMergeTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @usd = create(:asset, :usd)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum, color: '#627EEA')
    sign_in @user
  end

  test 'with two mergeable bots the page carries the Merge button and the bar' do
    basket([@btc])
    basket([@eth])

    get bots_path

    assert_response :success
    assert_select '[data-controller~="bots-merge"]', count: 1
    wrapper = css_select('[data-controller~="bots-merge"]').first
    assert_equal [@exchange.id], JSON.parse(wrapper['data-bots-merge-connected-value'])
    assert_equal({ @exchange.id.to_s => @exchange.name }, JSON.parse(wrapper['data-bots-merge-names-value']))
    # The lines keep their I18n tokens for the controller to fill in, not format strings of this test.
    # rubocop:disable Style/FormatStringToken
    assert_equal I18n.t('bot.merge.no_partner', quote: '%{quote}'), wrapper['data-bots-merge-no-partner-value']
    assert_equal I18n.t('bot.merge.no_shared_exchange', quote: '%{quote}'),
                 wrapper['data-bots-merge-no-shared-exchange-value']
    # rubocop:enable Style/FormatStringToken
    assert_select '.page-head--bots .dropdown-wrapper .dropdown button.dropdown__item[data-action="bots-merge#enter"]',
                  count: 1, text: I18n.t('button.merge')
    assert_select '[data-controller~="bots-merge"] .merge-bar[hidden][data-bots-merge-target="bar"]', count: 1
    assert_select '.merge-bar [data-bots-merge-target="hint"]', text: I18n.t('bot.merge.pick_hint')
    # Why a lone pick cannot merge: in the hint's place and style, in red, with no chips beside it.
    assert_select '.merge-bar .merge-bar__hint.text-danger[data-bots-merge-target="alone"][hidden]', count: 1
    assert_select '.merge-bar .ticker-group.ticker-group--fold[data-controller="ticker-fold"][data-bots-merge-target="stack"]',
                  count: 1
    assert_select '.merge-bar .merge-bar__warning[data-bots-merge-target="warning"][hidden]', count: 1
    assert_select '.merge-bar button[data-action="bots-merge#cancel"]', text: I18n.t('button.cancel')
    assert_select ".merge-bar form[action='#{new_bots_merge_path}'][method=get][data-turbo-frame=modal]", count: 1
    assert_select '.merge-bar form [data-bots-merge-target="ids"]', count: 1
    assert_select '.merge-bar form button[disabled][data-bots-merge-target="confirm"]', text: I18n.t('button.merge')
    # The drag stays on the grid; the merge controller wraps it rather than replacing it.
    assert_select '.itiles[data-controller~="bots-reorder"]', count: 1
  end

  test 'each tile says whether it can join, with what, and carries its members as chips' do
    anchor = basket([@btc, @eth])
    signal = create(:signal_bot, user: @user, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    archived = basket([@btc], status: :archived)
    executing = basket([@eth], status: :executing)
    kraken = create(:kraken_exchange)
    create(:ticker, exchange: kraken, base_asset: @btc, quote_asset: @usd)
    create(:transaction, :open, bot: anchor, exchange: @exchange, base: 'BTC', quote: 'USD')

    get bots_path

    tile = "#tile_bots_dca_multi_asset_#{anchor.id}"
    assert_select "#{tile}[data-merge-ok=true][data-merge-exchange='#{@exchange.id}'][data-merge-quote='#{@usd.id}']" \
                  "[data-merge-quote-symbol='USD'][data-merge-open-orders=true]"
    assert_select "#{tile} template[data-merge-chips]", count: 1
    chips = css_select("#{tile} template[data-merge-chips]").first.inner_html
    assert_includes chips, %(data-ticker-asset-id="#{@btc.id}")
    assert_includes chips, %(data-ticker-asset-id="#{@eth.id}")
    assert_includes chips, 'class="ticker"'
    assert_includes chips, 'style="background: #' # the asset colour lands on the chip
    # Where each member trades at this quote: BTC on both venues, ETH on Binance only.
    assert_includes chips, %(data-exchanges="#{[@exchange.id, kraken.id].sort.join(',')}")
    assert_includes chips, %(data-exchanges="#{@exchange.id}")

    assert_select "#tile_bots_signal_#{signal.id}[data-merge-ok=false][data-merge-open-orders=false]"
    assert_select "#tile_bots_signal_#{signal.id} template[data-merge-chips]", count: 0
    assert_select "#tile_bots_dca_multi_asset_#{executing.id}[data-merge-ok=false]"

    get bots_path(filter: 'archived')
    assert_select "#tile_bots_dca_multi_asset_#{archived.id}[data-merge-ok=false]"
    assert_select 'button[data-action="bots-merge#enter"]', count: 0
  end

  test 'an index bot with a composition can join; one that never derived cannot' do
    derived = create(:dca_index, user: @user, exchange: @exchange, quote_asset: @usd, status: :stopped, with_api_key: true)
    ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    BotIndexAsset.create!(bot: derived, asset: @btc, ticker:, in_index: true, target_allocation: 1, entered_at: 1.day.ago)
    bare = create(:dca_index, user: @user, exchange: @exchange, quote_asset: @usd, status: :stopped, with_api_key: true)

    get bots_path

    assert_select "#tile_bots_dca_index_#{derived.id}[data-merge-ok=true]"
    assert_select "#tile_bots_dca_index_#{derived.id} template[data-merge-chips] .ticker", count: 1
    assert_select "#tile_bots_dca_index_#{bare.id}[data-merge-ok=false]"
  end

  test 'with fewer than two mergeable bots there is no Merge button and no bar' do
    basket([@btc])
    create(:signal_bot, user: @user, exchange: @exchange, base_asset: @eth, quote_asset: @usd)

    get bots_path

    assert_response :success
    assert_select 'button[data-action="bots-merge#enter"]', count: 0
    assert_select '.merge-bar', count: 0
  end

  private

  def basket(assets, **attrs)
    create(:dca_multi_asset, user: @user, exchange: @exchange, quote_asset: @usd, base_assets: assets,
                             status: :stopped, **attrs)
  end
end
