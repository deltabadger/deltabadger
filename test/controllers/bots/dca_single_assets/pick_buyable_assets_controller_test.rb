require 'test_helper'

# The DCA wizard's asset step, shared by the single- and multi-asset bots: picking stays on the
# step, the basket is collected in a table under the sentence, and Next commits it — one asset
# continues as a DcaSingleAsset, more as a DcaMultiAsset.
class Bots::DcaSingleAssets::PickBuyableAssetsControllerTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true) # platform requires an admin to exist before bot flows render
    @user = create(:user, setup_completed: true)
    @binance = create(:binance_exchange)
    @usd = create(:asset, :usd)
    sign_in @user
    # Asset-first is the optional route, but it is where the basket mechanics under test are
    # easiest to drive (and where stock routing lives), so these tests opt into it.
    post bots_dca_single_assets_order_path, params: { flow: 'asset_first' }
  end

  # ── listing ──────────────────────────────────────────────────────────────────

  test 'lists an available base asset with the exchanges it trades on' do
    kraken = create(:kraken_exchange)
    eth = listed(:ethereum)
    create(:ticker, exchange: kraken, base_asset: eth, quote_asset: @usd)

    get step_path
    assert_response :ok
    assert_match 'ETH', response.body
    assert_match 'title="Binance"', response.body
    assert_match 'title="Kraken"', response.body
  end

  # The binance_us → binance collapse must survive page-deferred exchange resolution:
  # binance_name has to come from "is Binance available with assets", NOT from the page rows
  # (a page can legitimately contain a binance_us asset but no binance asset).
  test 'collapses binance_us to Binance even when no Binance asset is on the page' do
    filler = create(:asset, symbol: 'FILL', name: 'Filler')
    create(:exchange_asset, exchange: @binance, asset: filler, available: true)

    binance_us = create(:binance_us_exchange)
    aaa = create(:asset, symbol: 'AAA', name: 'Alpha')
    create(:ticker, exchange: binance_us, base_asset: aaa, quote_asset: @usd)

    get step_path
    assert_response :ok
    assert_match 'AAA', response.body
    assert_match 'title="Binance"', response.body, 'binance_us should render under the Binance label'
    assert_no_match 'Binance.US', response.body
  end

  test 'paginates the asset list in pages of ASSET_PAGE_SIZE' do
    page_size = Bots::Searchable::ASSET_PAGE_SIZE
    (page_size + 5).times { |i| listed(symbol: "C#{i}", name: "Coin #{i}") }

    get step_path
    assert_response :ok
    assert_equal page_size, response.body.scan('data-arrow-keys-navigation-target="item"').size

    get step_path(offset: page_size)
    assert_response :ok
    assert_equal 5, response.body.scan('data-arrow-keys-navigation-target="item"').size
  end

  test 'a lazy page frame request still renders just the page partial' do
    22.times { |i| listed(symbol: "A#{i}", name: "Asset #{i}") }
    pick Asset.find_by!(symbol: 'A0')

    get step_path, params: { offset: 20 }, headers: { 'Turbo-Frame' => 'assets-page-20' }

    assert_response :ok
    assert_select 'turbo-frame#assets-page-20'
    assert_select '.bot-creation-layout', count: 0
  end

  # ── picking stays on the step ────────────────────────────────────────────────

  test 'picking an asset stays on the step, stores it as the single base, and renders it' do
    eth = listed(:ethereum)

    get step_path
    pick eth

    assert_redirected_to step_path
    assert_equal eth.id, settings['base_asset_id']
    assert_nil settings['base_asset_ids']

    follow_redirect!
    assert_select '.conversational__lead', text: 'Buy'
    assert_select '.conversational__assets .conversational__stack .ticker', text: 'ETH'
    assert_select '.conversational__assets form.ticker.active input[name="query"]', count: 1
    assert_select '.wizard-assets__row', count: 1
    assert_select '.wizard-assets__row .rbutton', count: 1
    assert_select 'button', text: I18n.t('button.next') do |buttons|
      assert_nil buttons.first['disabled']
    end
  end

  test 'the empty step has no rows, no Next, and the search input alone in the asset slot' do
    listed(:ethereum)

    get step_path

    assert_response :ok
    assert_select '.wizard-assets__row', count: 0
    assert_select 'button', text: I18n.t('button.next'), count: 0
    assert_select '.conversational__stack', count: 0
    assert_select '.conversational__assets form.ticker.active input[name="query"]', count: 1
  end

  test 'a second pick turns the base into a list in pick order; removing back to one restores the base; removing the last clears both' do
    eth = listed(:ethereum)
    sol = listed(symbol: 'SOL', name: 'Solana')

    pick eth
    pick sol
    assert_equal [eth.id, sol.id], settings['base_asset_ids']
    assert_nil settings['base_asset_id']

    remove eth
    assert_redirected_to step_path
    assert_equal sol.id, settings['base_asset_id']
    assert_nil settings['base_asset_ids']

    remove sol
    assert_nil settings['base_asset_id']
    assert_nil settings['base_asset_ids']
    follow_redirect!
    assert_response :ok
    assert_select '.wizard-assets__row', count: 0
  end

  test 'a duplicate, an asset outside the search scope, and the twenty-first are ignored' do
    btc = listed(:bitcoin)
    pick btc
    outside = create(:asset, symbol: 'OUT', name: 'Outside')
    pick outside
    assert_equal btc.id, settings['base_asset_id']

    pick btc
    assert_equal btc.id, settings['base_asset_id']

    additions = 20.times.map { |i| listed(symbol: "A#{i}", name: "Asset #{i}") }
    additions.first(19).each { |asset| pick asset }
    assert_equal Bots::DcaMultiAsset::MAX_ASSETS, settings['base_asset_ids'].size

    pick additions.last
    assert_equal Bots::DcaMultiAsset::MAX_ASSETS, settings['base_asset_ids'].size
    refute_includes settings['base_asset_ids'], additions.last.id
  end

  test 'with twenty assets the step renders the cap note, no search input, and Next' do
    assets = 21.times.map { |i| listed(symbol: "A#{i}", name: "Asset #{i}") }
    assets.first(20).each { |asset| pick asset }

    get step_path

    assert_response :ok
    assert_select '.conversational__assets form.ticker.active', count: 0
    assert_select '.text-inactive', text: I18n.t('bot.dca_multi_asset.max_assets_reached', max: 20)
    assert_select 'button', text: I18n.t('button.next') do |buttons|
      assert_nil buttons.first['disabled']
    end
  end

  test 'picking with an exchange already chosen keeps the exchange' do
    btc = listed(:bitcoin)
    create(:api_key, user: @user, exchange: @binance, key_type: :trading, status: :correct)
    get step_path
    post bots_dca_single_assets_order_path, params: { flow: 'exchange_first' }
    post bots_dca_single_assets_pick_exchange_path, params: { bots_dca_single_asset: { exchange_id: @binance.id } }

    pick btc

    assert_redirected_to step_path
    assert_equal @binance.id.to_s, session[:bot_config]['exchange_id'].to_s
    assert_equal btc.id, settings['base_asset_id']
  end

  test 'asset-first: an exchange left behind with an empty basket is neither shown nor carried into the next pick' do
    btc = listed(:bitcoin)
    eth = listed(:ethereum)
    other = create(:kraken_exchange)
    create(:ticker, exchange: other, base_asset: btc, quote_asset: @usd)
    create(:api_key, user: @user, exchange: other, key_type: :trading, status: :correct)
    pick btc
    advance
    post bots_dca_single_assets_pick_exchange_path, params: { bots_dca_single_asset: { exchange_id: other.id } }
    remove btc
    assert_equal other.id.to_s, session[:bot_config]['exchange_id'].to_s

    # An empty basket in asset-first is a fresh start: the leftover venue neither narrows the list
    # nor shows as a chip — the exchange slot is the order switch again.
    get step_path
    assert_match 'ETH', response.body
    assert_select '.conversational form[action=?]', bots_dca_single_assets_order_path
    assert_select '.conversational input[value=?]', other.name, count: 0

    pick eth
    assert_nil session[:bot_config]['exchange_id']
    advance
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
  end

  test 'the prerequisite bounce follows the basket: a basket of two with an invalid key lands on the multi API step' do
    btc = listed(:bitcoin)
    eth = listed(:ethereum)
    create(:api_key, user: @user, exchange: @binance, key_type: :trading, status: :correct)
    get step_path
    post bots_dca_single_assets_order_path, params: { flow: 'exchange_first' }
    post bots_dca_single_assets_pick_exchange_path, params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    pick btc
    pick eth
    ApiKey.any_instance.stubs(:correct?).returns(false)

    get step_path
    assert_redirected_to new_bots_dca_multi_assets_add_api_key_path

    ApiKey.any_instance.unstub(:correct?)
    remove eth
    ApiKey.any_instance.stubs(:correct?).returns(false)
    get step_path
    assert_redirected_to new_bots_dca_single_assets_add_api_key_path
  end

  # ── Next ─────────────────────────────────────────────────────────────────────

  test 'Next with nothing chosen stays on the step' do
    create(:api_key, user: @user, exchange: @binance, key_type: :trading, status: :correct)
    get step_path
    post bots_dca_single_assets_order_path, params: { flow: 'exchange_first' }
    post bots_dca_single_assets_pick_exchange_path, params: { bots_dca_single_asset: { exchange_id: @binance.id } }

    advance

    assert_redirected_to step_path
  end

  test 'Next with one crypto asset continues as a single bot to the exchange step' do
    btc = listed(:bitcoin)
    pick btc

    advance

    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
    assert_equal btc.id, settings['base_asset_id']
    assert_nil settings['base_asset_ids']
  end

  test 'Next with two assets continues as a multi bot to the exchange step' do
    btc = listed(:bitcoin)
    eth = listed(:ethereum)
    pick btc
    pick eth

    advance

    assert_redirected_to new_bots_dca_multi_assets_pick_exchange_path
    assert_equal [btc.id, eth.id], settings['base_asset_ids']
    assert_nil settings['base_asset_id']
  end

  test 'Next with the exchange and key already in place lands on the spendable step, not the exchange step' do
    btc = listed(:bitcoin)
    eth = listed(:ethereum)
    create(:api_key, user: @user, exchange: @binance, key_type: :trading, status: :correct)
    pick btc
    advance
    post bots_dca_single_assets_pick_exchange_path, params: { bots_dca_single_asset: { exchange_id: @binance.id } }

    # Back on the step (the stack link), Next skips what is still complete.
    get step_path
    advance
    assert_redirected_to new_bots_dca_single_assets_pick_spendable_asset_path

    # The same for a basket that grew into a multi bot.
    pick eth
    advance
    assert_redirected_to new_bots_dca_multi_assets_pick_spendable_asset_path
    assert_equal @binance.id.to_s, session[:bot_config]['exchange_id'].to_s
  end

  test 'a combination no venue lists shows the warning, keeps the list, and disables Next; Next refuses it' do
    make_globally_disjoint

    get step_path
    assert_select '.wizard-assets__warning[role="alert"]', text: I18n.t('bot.dca_multi_asset.no_common_exchange')
    assert_select '.wizard-assets__row', count: 2
    assert_select '.wizard-assets__row .rbutton', count: 2
    assert_select 'button[disabled]', text: I18n.t('button.next')

    advance
    assert_redirected_to step_path
    assert_equal I18n.t('bot.dca_multi_asset.no_common_exchange'), flash[:alert]
  end

  test 'a chosen exchange that does not carry the basket warns even though another venue would' do
    btc = listed(:bitcoin)
    eth = listed(:ethereum)
    other = create(:kraken_exchange)
    [btc, eth].each { |asset| create(:ticker, exchange: other, base_asset: asset, quote_asset: @usd) }
    pick btc
    pick eth
    advance
    post bots_dca_multi_assets_pick_exchange_path, params: { bots_dca_multi_asset: { exchange_id: @binance.id } }
    Ticker.find_by!(exchange: @binance, base_asset: eth, quote_asset: @usd).update!(available: false)

    get step_path

    assert_select '.wizard-assets__warning[role="alert"]', text: I18n.t('bot.dca_multi_asset.no_common_exchange')
    assert_select 'button[disabled]', text: I18n.t('button.next')
    # The chosen exchange stays re-pickable from the chip — recovery without emptying the basket.
    assert_select '.conversational form[action=?] input[name="to"][value="exchange"]', advance_path
  end

  test 'a retired exchange left in the session does not count as supported' do
    btc = listed(:bitcoin)
    eth = listed(:ethereum)
    retired = create(:bitmart_exchange)
    [btc, eth].each { |asset| create(:ticker, exchange: retired, base_asset: asset, quote_asset: @usd) }
    pick btc
    pick eth
    advance
    post bots_dca_multi_assets_pick_exchange_path, params: { bots_dca_multi_asset: { exchange_id: retired.id } }

    get step_path

    assert_select '.wizard-assets__warning[role="alert"]', text: I18n.t('bot.dca_multi_asset.no_common_exchange')
    assert_select 'button[disabled]', text: I18n.t('button.next')
  end

  # ── stocks (asset-first routes through the broker step) ──────────────────────

  test 'Next with one stock asset and a single broker auto-selects it and skips to the api-key step' do
    alpaca = create(:alpaca_exchange)
    aapl = stock('AAPL', 'Apple Inc', alpaca)
    pick aapl

    advance

    assert_redirected_to new_bots_dca_single_assets_add_api_key_path
    assert_equal alpaca.id.to_s, session[:bot_config]['exchange_id'].to_s
    assert_equal @usd.id, settings['quote_asset_id']
  end

  test 'Next with one stock asset and two brokers routes to the broker picker' do
    alpaca = create(:alpaca_exchange)
    ibkr = create(:ibkr_exchange)
    aapl = stock('AAPL', 'Apple Inc', alpaca, ibkr)
    pick aapl

    advance

    assert_redirected_to new_bots_dca_single_assets_pick_stock_broker_path
  end

  test 'Next keeps an already chosen broker instead of reopening the picker (single and multi)' do
    alpaca = create(:alpaca_exchange)
    ibkr = create(:ibkr_exchange)
    aapl = stock('AAPL', 'Apple Inc', alpaca, ibkr)
    msft = stock('MSFT', 'Microsoft', alpaca, ibkr)
    create(:api_key, user: @user, exchange: ibkr, key_type: :trading, status: :correct)
    pick aapl
    advance
    post bots_dca_single_assets_pick_stock_broker_path, params: { bots_dca_single_asset: { exchange_id: ibkr.id } }
    assert_redirected_to new_bots_dca_single_assets_add_api_key_path

    get step_path
    advance
    assert_redirected_to new_bots_dca_single_assets_pick_spendable_asset_path
    assert_equal ibkr.id.to_s, session[:bot_config]['exchange_id'].to_s

    pick msft
    advance
    assert_redirected_to new_bots_dca_multi_assets_pick_spendable_asset_path
    assert_equal ibkr.id.to_s, session[:bot_config]['exchange_id'].to_s
  end

  # ── sentence ─────────────────────────────────────────────────────────────────

  test 'the exchange slot switches the order while nothing is chosen and advances once something is' do
    btc = listed(:bitcoin)

    get step_path
    assert_select '.conversational form[action=?]', bots_dca_single_assets_order_path
    assert_select '.conversational form[action=?]', advance_path, count: 0

    pick btc
    get step_path
    assert_select '.conversational form[action=?]', bots_dca_single_assets_order_path, count: 0
    assert_select '.conversational form[action=?]', advance_path
  end

  test 'on the step the chosen exchange chip advances to the exchange step of the decided kind' do
    btc = listed(:bitcoin)
    eth = listed(:ethereum)
    create(:api_key, user: @user, exchange: @binance, key_type: :trading, status: :correct)
    get step_path
    post bots_dca_single_assets_order_path, params: { flow: 'exchange_first' }
    post bots_dca_single_assets_pick_exchange_path, params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    pick btc

    get step_path
    assert_select '.conversational form[action=?] input[name="to"][value="exchange"]', advance_path
    assert_select '.conversational form[action=?]', new_bots_dca_single_assets_pick_exchange_path, count: 0

    post advance_path, params: { to: 'exchange' }
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path

    pick eth
    post advance_path, params: { to: 'exchange' }
    assert_redirected_to new_bots_dca_multi_assets_pick_exchange_path
    assert_equal [btc.id, eth.id], settings['base_asset_ids']
  end

  test 'away from the step the basket is one stack linking back to it' do
    btc = listed(:bitcoin)
    eth = listed(:ethereum)
    sol = listed(symbol: 'SOL', name: 'Solana')
    [btc, eth, sol].each { |asset| pick asset }
    advance
    follow_redirect!

    assert_response :ok
    assert_select '.conversational__lead', text: 'Buy'
    assert_select '.conversational__lead', text: /Invest/, count: 0
    assert_select 'a.conversational__stack[href=?]', step_path, count: 1 do
      assert_select '.ticker', count: 3
    end
    assert_select '.wizard-asset-list', count: 0
  end

  test 'away from the step a single asset is still one chip linking back to it' do
    btc = listed(:bitcoin)
    pick btc
    advance
    follow_redirect!

    assert_select 'a.conversational__stack[href=?]', step_path, count: 1 do
      assert_select '.ticker', text: 'BTC', count: 1
    end
  end

  private

  def step_path(**params) = new_bots_dca_single_assets_pick_buyable_asset_path(**params)
  def advance_path = advance_bots_dca_single_assets_pick_buyable_asset_path
  def settings = session[:bot_config].fetch('settings', {})

  def pick(asset)
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: asset.id } }
  end

  def remove(asset)
    post remove_bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: asset.id } }
  end

  def advance = post advance_path

  def listed(*traits, exchange: @binance, **attrs)
    asset = create(:asset, *traits, **attrs)
    create(:ticker, exchange: exchange, base_asset: asset, quote_asset: @usd)
    asset
  end

  def stock(symbol, name, *venues)
    asset = create(:asset, symbol: symbol, name: name, category: 'Stock', external_id: symbol.downcase)
    venues.each { |venue| create(:ticker, exchange: venue, base_asset: asset, quote_asset: @usd, base: symbol, quote: 'USD') }
    asset
  end

  def make_globally_disjoint
    btc = listed(:bitcoin)
    eth = listed(:ethereum)
    pick btc
    pick eth
    other = create(:kraken_exchange)
    create(:ticker, exchange: other, base_asset: eth, quote_asset: @usd)
    Ticker.find_by!(exchange: @binance, base_asset: eth, quote_asset: @usd).update!(available: false)
  end
end

# Sessions this code did not write: a pre-deploy one-element list, duplicates, a dead id, a stale
# quote. GET shows the canonical basket without touching the session; Next normalises it.
class Bots::DcaSingleAssets::PickBuyableAssetsStaleSessionTest < ActionController::TestCase
  tests Bots::DcaSingleAssets::PickBuyableAssetsController
  include Devise::Test::ControllerHelpers

  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @binance = create(:binance_exchange)
    @usd = create(:asset, :usd)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    create(:ticker, exchange: @binance, base_asset: @btc, quote_asset: @usd)
    create(:ticker, exchange: @binance, base_asset: @eth, quote_asset: @usd)
  end

  test 'a one-element list renders as one chosen asset, GET leaves it untouched, and Next writes the single shape' do
    get :new, session: { bot_config: { 'flow' => 'asset_first', 'settings' => { 'base_asset_ids' => [@btc.id] } } }

    assert_response :ok
    assert_select '.wizard-assets__row', count: 1
    assert_equal [@btc.id], session[:bot_config]['settings']['base_asset_ids']
    assert_nil session[:bot_config]['settings']['base_asset_id']

    post :advance
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
    assert_equal @btc.id, session[:bot_config]['settings']['base_asset_id']
    assert_nil session[:bot_config]['settings']['base_asset_ids']
  end

  test 'the exchange chip hop normalises a one-element list before the single exchange step' do
    post :advance, params: { to: 'exchange' },
                   session: { bot_config: { 'flow' => 'asset_first', 'settings' => { 'base_asset_ids' => [@btc.id] } } }

    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
    assert_equal @btc.id, session[:bot_config]['settings']['base_asset_id']
    assert_nil session[:bot_config]['settings']['base_asset_ids']
  end

  test 'a duplicate and a dead id are dropped, and writes scrub the legacy dual keys' do
    get :new, session: { bot_config: { 'flow' => 'asset_first', 'settings' => {
      'base_asset_ids' => [@btc.id, @btc.id, 999_999], 'base0_asset_id' => @btc.id, 'base1_asset_id' => @eth.id
    } } }

    assert_response :ok
    assert_select '.wizard-assets__row', count: 1
    assert_select 'button', text: I18n.t('button.next') do |buttons|
      assert_nil buttons.first['disabled']
    end

    post :advance
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
    assert_equal @btc.id, session[:bot_config]['settings']['base_asset_id']
    assert_nil session[:bot_config]['settings']['base_asset_ids']
    assert_nil session[:bot_config]['settings']['base0_asset_id']
    assert_nil session[:bot_config]['settings']['base1_asset_id']
  end

  test 'a stale quote neither narrows the step nor survives Next' do
    eur = create(:asset, symbol: 'EUR', name: 'Euro', category: 'Fiat', external_id: 'eur')

    get :new, session: { bot_config: { 'flow' => 'asset_first',
                                       'settings' => { 'base_asset_ids' => [@btc.id, @eth.id], 'quote_asset_id' => eur.id } } }

    assert_response :ok
    assert_select '.wizard-assets__warning', count: 0
    assert_select 'button', text: I18n.t('button.next') do |buttons|
      assert_nil buttons.first['disabled']
    end

    post :advance
    assert_redirected_to new_bots_dca_multi_assets_pick_exchange_path
    assert_nil session[:bot_config]['settings']['quote_asset_id']
    assert_equal [@btc.id, @eth.id], session[:bot_config]['settings']['base_asset_ids']
  end
end
