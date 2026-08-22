require 'test_helper'

# Order of the bot-creation wizard. Exchange-first is the DEFAULT: the venue is
# picked before the assets; asset-first is the optional route taken by flipping
# the order on the first step. Covers the toggle (POST-only), both happy paths for
# single + multi, downstream reset, the order-aware prerequisite bounce, and the
# stock-venue path (exchange-first must NOT route through the asset-first
# StockBrokerRoutable machinery).
class Bots::ExchangeFirstCreationTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @kraken = create(:kraken_exchange)
    @bitcoin = create(:asset, :bitcoin)
    @ethereum = create(:asset, :ethereum)
    @usd = create(:asset, :usd)
    create(:ticker, exchange: @binance, base_asset: @bitcoin, quote_asset: @usd)
    create(:ticker, exchange: @binance, base_asset: @ethereum, quote_asset: @usd)
    create(:ticker, exchange: @kraken, base_asset: @bitcoin, quote_asset: @usd)
    create(:api_key, user: @user, exchange: @binance, key_type: :trading, status: :correct)
    create(:api_key, user: @user, exchange: @kraken, key_type: :trading, status: :correct)

    sign_in @user
    Bot::ActionJob.stubs(:perform_later)
  end

  def switch_to_exchange_first
    get new_bots_dca_single_assets_pick_exchange_path
    post bots_dca_single_assets_order_path, params: { flow: 'exchange_first' }
  end

  def switch_to_asset_first
    get new_bots_dca_single_assets_pick_exchange_path
    post bots_dca_single_assets_order_path, params: { flow: 'asset_first' }
  end

  def pick(asset)
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: asset.id } }
  end

  def advance(**params)
    post advance_bots_dca_single_assets_pick_buyable_asset_path, params: params
  end

  # Exchange-first up to the asset step with the venue and key in place.
  def reach_asset_step(exchange = @binance)
    switch_to_exchange_first
    follow_redirect!
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: exchange.id } }
    follow_redirect! # add_api_key (key valid in dry-run)
    follow_redirect! # pick_buyable
  end

  # ── the default order ────────────────────────────────────────────────────────

  test 'the default order is exchange-first: a fresh session starts at the exchange step' do
    get new_bots_dca_single_assets_pick_exchange_path
    assert_response :ok
    assert_select 'div.process-progress h4', 'Pick exchange'
    # With nothing to narrate yet, the sentence only offers the optional asset-first route.
    assert_select '.conversational .conversational__lead', text: 'You can also start with'
    assert_select '.conversational .conversational__lead', text: 'Buy', count: 0
    assert_select '.conversational form[action=?] input[name="flow"][value="asset_first"]',
                  bots_dca_single_assets_order_path
    assert_select '.conversational form[action=?] input[placeholder="assets"]', bots_dca_single_assets_order_path

    # The asset step is not reachable before the venue.
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
  end

  # ── header titles (no segmented toggle; the sentence does the switching) ─────

  test 'the header reads Pick asset / Pick exchange and there is no segmented toggle' do
    # Asset-first: the first step is the asset step.
    switch_to_asset_first
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok
    assert_select '.order-toggle', false, 'the header toggle is removed'
    assert_select 'div.process-progress h4', 'Pick asset'

    # The exchange step reads "Pick exchange" to parallel "Pick asset".
    pick @bitcoin
    advance
    get new_bots_dca_single_assets_pick_exchange_path
    assert_response :ok
    assert_select 'div.process-progress h4', 'Pick exchange'
  end

  test 'exchange-first first step header reads Pick exchange' do
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_order_path, params: { flow: 'exchange_first' }
    follow_redirect!
    assert_response :ok
    assert_select 'div.process-progress h4', 'Pick exchange'
    assert_select '.order-toggle', false
  end

  test 'the spending slot appears only after an asset and an exchange are chosen' do
    # First step, nothing chosen yet — no "spending".
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_select '.conversational .conversational__lead', text: 'spending', count: 0

    # With an asset and an exchange both chosen, the spending slot is shown.
    pick @bitcoin
    advance
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    get new_bots_dca_single_assets_pick_spendable_asset_path
    assert_select '.conversational .conversational__lead', text: 'spending'
  end

  test 'on the first step a conversational slot switches the order; after a pick it advances instead' do
    # Asset-first first step: the (unfilled) exchange slot is a mode switch.
    switch_to_asset_first
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_select '.conversational form[action=?]', bots_dca_single_assets_order_path

    # Once an asset is picked the user stays on the step; the exchange slot now moves on.
    pick @bitcoin
    follow_redirect!
    assert_select '.conversational form[action=?]', bots_dca_single_assets_order_path, count: 0
    assert_select '.conversational form[action=?]', advance_bots_dca_single_assets_pick_buyable_asset_path

    # And on the exchange step it is the real picker — no switch.
    advance
    follow_redirect!
    assert_select '.conversational form[action=?]', bots_dca_single_assets_order_path, count: 0
  end

  test 'exchange-first first step: the unfilled asset slot switches back to asset-first' do
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_order_path, params: { flow: 'exchange_first' }
    follow_redirect! # exchange picker (first step in exchange-first)
    assert_response :ok
    assert_select '.conversational form[action=?]', bots_dca_single_assets_order_path
  end

  test 'exchange-first: the unfilled asset slot still switches on the API-key step' do
    # Key not valid → the API-key step actually renders (rather than skipping).
    ApiKey.any_instance.stubs(:correct?).returns(false)
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_order_path, params: { flow: 'exchange_first' }
    follow_redirect! # exchange picker
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    follow_redirect! # add_api_key step
    assert_response :ok
    # The asset is still unpicked here, so its slot remains an order switch.
    assert_select '.conversational form[action=?]', bots_dca_single_assets_order_path
  end

  test 'the order switch is POST-only so a Turbo hover-prefetch cannot flip it' do
    routes = Rails.application.routes
    assert_equal({ controller: 'bots/dca_single_assets/orders', action: 'create' },
                 routes.recognize_path(bots_dca_single_assets_order_path, method: :post))
    assert_raises(ActionController::RoutingError) do
      routes.recognize_path(bots_dca_single_assets_order_path, method: :get)
    end
  end

  test 'posting the order switch flips the flow and redirects to the new first step' do
    get new_bots_dca_single_assets_pick_buyable_asset_path

    post bots_dca_single_assets_order_path, params: { flow: 'exchange_first' }
    assert_equal 'exchange_first', session[:bot_config]['flow']
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path

    post bots_dca_single_assets_order_path, params: { flow: 'asset_first' }
    assert_equal 'asset_first', session[:bot_config]['flow']
    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
  end

  # ── single exchange-first happy path ─────────────────────────────────────────

  test 'single exchange-first: exchange → api → asset → Next → quote creates the bot' do
    switch_to_exchange_first
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
    follow_redirect!

    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    assert_redirected_to new_bots_dca_single_assets_add_api_key_path
    follow_redirect!
    # Key already valid (dry-run) → exchange-first advances to the ASSET step.
    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
    follow_redirect!
    assert_response :ok

    # Picking stays on the step; Next moves on to the quote.
    pick @bitcoin
    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
    advance
    assert_redirected_to new_bots_dca_single_assets_pick_spendable_asset_path
    follow_redirect!
    assert_response :ok

    assert_difference 'Bots::DcaSingleAsset.count', 1 do
      post bots_dca_single_assets_pick_spendable_asset_path,
           params: { bots_dca_single_asset: { quote_asset_id: @usd.id } }, as: :turbo_stream
    end

    bot = Bots::DcaSingleAsset.last
    assert_equal @bitcoin, bot.base_asset
    assert_equal @usd, bot.quote_asset
    assert_equal @binance, bot.exchange
    assert_predicate bot, :created?
    # flow is ephemeral wizard state — never persisted on the bot.
    refute bot.settings.key?('flow')
  end

  # ── multi exchange-first happy path ──────────────────────────────────────────

  test 'multi exchange-first keeps the exchange while adding assets and creates the bot' do
    reach_asset_step

    pick @bitcoin
    pick @ethereum
    assert_equal 'exchange_first', session[:bot_config]['flow']
    assert_equal @binance.id.to_s, session[:bot_config]['exchange_id'].to_s
    assert_equal [@bitcoin.id, @ethereum.id], session[:bot_config].dig('settings', 'base_asset_ids')
    assert_nil session[:bot_config].dig('settings', 'quote_asset_id')

    advance
    assert_redirected_to new_bots_dca_multi_assets_pick_spendable_asset_path

    assert_difference 'Bots::DcaMultiAsset.count', 1 do
      post bots_dca_multi_assets_pick_spendable_asset_path,
           params: { bots_dca_multi_asset: { quote_asset_id: @usd.id } }, as: :turbo_stream
    end

    bot = Bots::DcaMultiAsset.last
    assert_equal [@bitcoin.id, @ethereum.id], bot.base_asset_ids
    assert_equal @binance, bot.exchange
    assert_equal @usd, bot.quote_asset
    assert_predicate bot, :created?
  end

  # ── downstream reset ─────────────────────────────────────────────────────────

  test 'exchange-first: re-picking the exchange keeps the chosen asset and only re-asks the exchange' do
    reach_asset_step
    pick @bitcoin
    assert_equal @bitcoin.id.to_s, session[:bot_config].dig('settings', 'base_asset_id').to_s

    # Go back and re-pick a different exchange (Kraken also lists BTC). The asset
    # is the anchor — it survives; the exchange is swapped and the quote dropped.
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @kraken.id } }
    assert_equal @kraken.id.to_s, session[:bot_config]['exchange_id'].to_s
    assert_equal @bitcoin.id.to_s, session[:bot_config].dig('settings', 'base_asset_id').to_s,
                 'the chosen asset must survive an exchange re-pick'
    assert_nil session[:bot_config].dig('settings', 'quote_asset_id')

    # And the wizard does not re-ask the asset: after the key it lands on spendable.
    follow_redirect! # add_api_key (Kraken key valid in dry-run)
    follow_redirect! # → pick_spendable, NOT the asset step
    assert_equal new_bots_dca_single_assets_pick_spendable_asset_path, request.path
  end

  test 'exchange-first: the exchange chip on the asset step re-picks for two assets (multi) and for one (single), keeping the basket' do
    reach_asset_step
    pick @bitcoin
    pick @ethereum

    # Two assets: the chip hops through advance into the multi exchange step, scoped to venues
    # carrying both (Binance only).
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_select '.conversational form[action=?] input[name="to"][value="exchange"]',
                  advance_bots_dca_single_assets_pick_buyable_asset_path
    advance(to: 'exchange')
    assert_redirected_to new_bots_dca_multi_assets_pick_exchange_path
    follow_redirect!
    assert_response :ok
    assert_select "button.exchange-grid__item[value='#{@binance.id}']"
    assert_select "button.exchange-grid__item[value='#{@kraken.id}']", count: 0
    post bots_dca_multi_assets_pick_exchange_path,
         params: { bots_dca_multi_asset: { exchange_id: @binance.id } }
    follow_redirect! # add_api_key
    follow_redirect! # multi pick_spendable: the basket survived
    assert_equal new_bots_dca_multi_assets_pick_spendable_asset_path, request.path
    assert_equal [@bitcoin.id, @ethereum.id], session[:bot_config].dig('settings', 'base_asset_ids')

    # Back to one asset: the single exchange step (Kraken lists BTC too).
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post remove_bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @ethereum.id } }
    advance(to: 'exchange')
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @kraken.id } }
    follow_redirect! # add_api_key
    follow_redirect! # pick_spendable: the asset survived
    assert_equal new_bots_dca_single_assets_pick_spendable_asset_path, request.path
    assert_equal @bitcoin.id, session[:bot_config].dig('settings', 'base_asset_id')
  end

  test 'exchange-first: re-opening the exchange picker shows the chosen asset as a chip, not a doubled empty slot' do
    reach_asset_step
    pick @bitcoin

    # Re-open the exchange picker.
    get new_bots_dca_single_assets_pick_exchange_path
    assert_response :ok
    # The chosen asset renders as a chip linking back to the asset step (not an empty switch placeholder).
    assert_select '.conversational a.conversational__stack .ticker', text: 'BTC'
    # And there is no empty asset switch placeholder doubling it.
    assert_select '.conversational .ticker--switch', false
  end

  # ── order-aware prerequisite bounce ─────────────────────────────────────────

  test 'exchange-first: a direct GET to the asset step bounces to the first incomplete step' do
    switch_to_exchange_first # flow set, everything wiped
    # In exchange-first the asset step is third; with no exchange/key it bounces.
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
  end

  test 'asset-first still bounces the exchange step back to the asset step' do
    switch_to_asset_first
    get new_bots_dca_single_assets_pick_exchange_path
    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
  end

  # ── exchange-first stock venue (no StockBrokerRoutable) ──────────────────────

  test 'exchange-first stock: pick the venue first, then a stock, without the broker picker' do
    alpaca = create(:alpaca_exchange)
    aapl = create(:asset, symbol: 'AAPL', name: 'Apple Inc', category: 'Stock', external_id: 'aapl')
    create(:ticker, exchange: alpaca, base_asset: aapl, quote_asset: @usd, base: 'AAPL', quote: 'USD')
    create(:api_key, user: @user, exchange: alpaca, key_type: :trading, status: :correct)

    reach_asset_step(alpaca) # listing only this venue's stocks
    assert_response :ok
    assert_match 'AAPL', response.body

    # Picking the stock stays; Next must NOT bounce to the broker picker — the venue is set.
    pick aapl
    advance
    assert_redirected_to new_bots_dca_single_assets_pick_spendable_asset_path
    assert_equal alpaca.id.to_s, session[:bot_config]['exchange_id'].to_s, 'chosen venue must be preserved'
    follow_redirect!
    assert_response :ok

    assert_difference 'Bots::DcaSingleAsset.count', 1 do
      post bots_dca_single_assets_pick_spendable_asset_path,
           params: { bots_dca_single_asset: { quote_asset_id: @usd.id } }, as: :turbo_stream
    end

    bot = Bots::DcaSingleAsset.last
    assert_equal aapl, bot.base_asset
    assert_equal alpaca, bot.exchange
    assert_equal @usd, bot.quote_asset
  end

  # ── empty / syncing catalog ──────────────────────────────────────────────────

  test 'exchange-first: a chosen venue with no synced assets shows the syncing notice' do
    reach_asset_step # pick_buyable (assets present)

    # Simulate the venue's catalog not being synced yet (self-hosted Alpaca).
    Ticker.where(exchange: @binance).update_all(available: false)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok
    assert_match(/syncing/i, response.body)
  end

  test 'a zero-result search on a chosen venue is not mistaken for a syncing catalog' do
    reach_asset_step

    # A search that matches nothing must keep the picker (not show the syncing
    # notice) — the catalog is synced, the query just has no hits.
    get new_bots_dca_single_assets_pick_buyable_asset_path, params: { query: 'zzzznomatch' }
    assert_response :ok
    assert_no_match(/syncing/i, response.body)
  end

  test 'an exhausted add-more list is not mistaken for a syncing catalog either' do
    reach_asset_step
    pick @bitcoin
    pick @ethereum # every Binance asset is in the basket now

    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok
    assert_no_match(/syncing/i, response.body)
  end
end
