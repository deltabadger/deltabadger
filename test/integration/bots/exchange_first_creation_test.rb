require 'test_helper'

# Exchange-first variant of the bot-creation wizard: the user flips the order on
# the first step and picks the venue before the asset. Covers the toggle (POST-
# only), the reversed happy paths for single + dual, downstream reset, the
# order-aware prerequisite bounce, and the stock-venue path (which must NOT route
# through the asset-first StockBrokerRoutable machinery).
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
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_order_path, params: { flow: 'exchange_first' }
  end

  # ── header titles (no segmented toggle; the sentence does the switching) ─────

  test 'the header reads Pick asset / Pick exchange and there is no segmented toggle' do
    # Asset-first: the first step is the asset step.
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok
    assert_select '.order-toggle', false, 'the header toggle is removed'
    assert_select 'div.process-progress h4', 'Pick asset'

    # The exchange step reads "Pick exchange" to parallel "Pick asset".
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @bitcoin.id } }
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
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @bitcoin.id } }
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    get new_bots_dca_single_assets_pick_spendable_asset_path
    assert_select '.conversational .conversational__lead', text: 'spending'
  end

  test 'on the first step a conversational slot switches the order; after a pick it does not' do
    # Asset-first first step: the (unfilled) exchange slot is a mode switch.
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_select '.conversational form[action=?]', bots_dca_single_assets_order_path

    # Once the asset is picked, the exchange slot becomes the real picker — no switch.
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @bitcoin.id } }
    get new_bots_dca_single_assets_pick_exchange_path
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

  test 'single exchange-first: exchange → api → asset → quote creates the bot' do
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

    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @bitcoin.id } }
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

  # ── dual exchange-first happy path (promotion-only) ─────────────────────────

  test 'dual exchange-first: exchange → api → base0 → promote → base1 → quote creates the bot' do
    switch_to_exchange_first
    follow_redirect!

    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    follow_redirect! # add_api_key
    follow_redirect! # → pick_buyable (base0 on the single picker)
    assert_response :ok

    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @bitcoin.id } }
    assert_redirected_to new_bots_dca_single_assets_pick_spendable_asset_path

    # Promote: the flow variant must survive the single→dual rewrite.
    post promote_to_dual_bots_dca_single_assets_pick_exchange_path
    assert_redirected_to new_bots_dca_dual_assets_pick_second_buyable_asset_path
    assert_equal 'exchange_first', session[:bot_config]['flow']
    assert_equal @bitcoin.id.to_s, session[:bot_config].dig('settings', 'base0_asset_id').to_s
    follow_redirect!
    assert_response :ok

    post bots_dca_dual_assets_pick_second_buyable_asset_path,
         params: { bots_dca_dual_asset: { base1_asset_id: @ethereum.id } }
    assert_redirected_to new_bots_dca_dual_assets_pick_spendable_asset_path
    follow_redirect!
    assert_response :ok

    assert_difference 'Bots::DcaDualAsset.count', 1 do
      post bots_dca_dual_assets_pick_spendable_asset_path,
           params: { bots_dca_dual_asset: { quote_asset_id: @usd.id } }, as: :turbo_stream
    end

    bot = Bots::DcaDualAsset.last
    assert_equal @bitcoin, bot.base0_asset
    assert_equal @ethereum, bot.base1_asset
    assert_equal @usd, bot.quote_asset
    assert_equal @binance, bot.exchange
    assert_predicate bot, :created?
  end

  # ── downstream reset ─────────────────────────────────────────────────────────

  test 'exchange-first: re-picking the exchange keeps the chosen asset and only re-asks the exchange' do
    switch_to_exchange_first
    follow_redirect!
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    follow_redirect! # add_api_key
    follow_redirect! # pick_buyable
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @bitcoin.id } }
    assert_redirected_to new_bots_dca_single_assets_pick_spendable_asset_path
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

  test 'exchange-first: re-opening the exchange picker shows the chosen asset as a filled chip, not a doubled empty slot' do
    switch_to_exchange_first
    follow_redirect!                                  # exchange picker
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    follow_redirect!                                  # add_api_key (valid in dry-run)
    follow_redirect!                                  # pick_buyable
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @bitcoin.id } }

    # Re-open the exchange picker (clicking the exchange chip).
    get new_bots_dca_single_assets_pick_exchange_path
    assert_response :ok
    # The chosen asset renders as a filled chip (not an empty switch placeholder).
    assert_select '.conversational .ticker.filled input[value=?]', @bitcoin.symbol
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
    get new_bots_dca_single_assets_pick_exchange_path
    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
  end

  # ── exchange-first stock venue (no StockBrokerRoutable) ──────────────────────

  test 'exchange-first stock: pick the venue first, then a stock, without the broker picker' do
    alpaca = create(:alpaca_exchange)
    aapl = create(:asset, symbol: 'AAPL', name: 'Apple Inc', category: 'Stock', external_id: 'aapl')
    create(:ticker, exchange: alpaca, base_asset: aapl, quote_asset: @usd, base: 'AAPL', quote: 'USD')
    create(:api_key, user: @user, exchange: alpaca, key_type: :trading, status: :correct)

    switch_to_exchange_first
    follow_redirect!

    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: alpaca.id } }
    follow_redirect! # add_api_key (key valid in dry-run)
    follow_redirect! # → pick_buyable, listing only this venue's stocks
    assert_response :ok
    assert_match 'AAPL', response.body

    # Picking the stock must NOT bounce to the broker picker — the venue is set.
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: aapl.id } }
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
    switch_to_exchange_first
    follow_redirect!
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    follow_redirect! # add_api_key
    follow_redirect! # pick_buyable (assets present)

    # Simulate the venue's catalog not being synced yet (self-hosted Alpaca).
    Ticker.where(exchange: @binance).update_all(available: false)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok
    assert_match(/syncing/i, response.body)
  end

  test 'a zero-result search on a chosen venue is not mistaken for a syncing catalog' do
    switch_to_exchange_first
    follow_redirect!
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    follow_redirect! # add_api_key
    follow_redirect! # pick_buyable

    # A search that matches nothing must keep the picker (not show the syncing
    # notice) — the catalog is synced, the query just has no hits.
    get new_bots_dca_single_assets_pick_buyable_asset_path, params: { query: 'zzzznomatch' }
    assert_response :ok
    assert_no_match(/syncing/i, response.body)
  end
end
