require 'test_helper'

class Rules::WithdrawalCreationTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    @bitcoin = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @binance = create(:binance_exchange)
    # Ticker is required for `available_exchanges_for_current_settings` —
    # the withdrawal wizard reuses the bot Searchable concern, which
    # filters exchanges to those that have a ticker for the picked asset.
    # The :ticker factory auto-creates an ExchangeAsset for base + quote
    # (see test/factories.rb), so update the existing row rather than
    # creating a second one (which would trip the uniqueness constraint).
    create(:ticker, exchange: @binance, base_asset: @bitcoin, quote_asset: @usd)
    ExchangeAsset.find_by!(exchange: @binance, asset: @bitcoin)
                 .update!(withdrawal_fee: '0.0005', withdrawal_fee_updated_at: Time.current)
    create(:api_key, user: @user, exchange: @binance, key_type: :withdrawal, status: :correct)
    sign_in @user
  end

  # ---- The flow opens on the exchange, not the asset ----

  test 'pick_exchange is the first step and renders the exchange grid' do
    get new_rules_withdrawals_pick_exchange_path
    assert_response :success

    assert_match(/Automate withdrawals from/, response.body,
                 'the opening step should ask which exchange to withdraw from')
    assert_match(/exchange-grid__item/, response.body,
                 'exchange picker should render the .exchange-grid__item cards')
    assert_no_match(/exchange-picker__item--header/, response.body,
                    'exchange picker should no longer render the old Maker/Taker header row')
    assert_no_match(/name="query"/, response.body,
                    'no search box on this step — the exchange is picked by clicking a tile')
  end

  test 'the asset step is unreachable before an exchange is picked' do
    get new_rules_withdrawals_pick_asset_path
    assert_redirected_to new_rules_withdrawals_pick_exchange_path
  end

  # ---- Asset picker uses .ticker.active on the search form ----

  # ---- Keys come right after the exchange, as in every other flow ----

  test 'picking the exchange moves on to the API key step' do
    get new_rules_withdrawals_pick_exchange_path
    post rules_withdrawals_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    assert_redirected_to new_rules_withdrawals_add_api_key_path
  end

  test 'without a key, the connect card shows before any asset is picked' do
    @user.api_keys.where(exchange: @binance).destroy_all
    # The test environment skips the key step outright (dry run); this is about the real path.
    with_dry_run(false) do
      get new_rules_withdrawals_pick_exchange_path
      post rules_withdrawals_pick_exchange_path,
           params: { bots_dca_single_asset: { exchange_id: @binance.id } }
      follow_redirect!
    end
    assert_response :success

    assert_select '.set-api--connect h2.set-api__title', text: "Connect #{@binance.name}"
    assert_select '.conversational .ticker', count: 0, message: 'no asset chip yet: the asset is picked after the keys'
    assert_select '.process-progress h4', text: I18n.t('bot.setup.progress_steps.api')
  end

  test 'with a working key already saved, the key step hands over to the asset step' do
    with_dry_run(false) do
      get new_rules_withdrawals_pick_exchange_path
      post rules_withdrawals_pick_exchange_path,
           params: { bots_dca_single_asset: { exchange_id: @binance.id } }
      get new_rules_withdrawals_add_api_key_path
    end
    assert_redirected_to new_rules_withdrawals_pick_asset_path
  end

  # ---- Asset picker uses .ticker.active on the search form ----

  test 'pick_asset step renders the search input as a .ticker.active form, not .sinput' do
    # Drive the wizard via real HTTP so session is populated naturally —
    # do not mutate session[] directly in integration tests.
    get new_rules_withdrawals_pick_exchange_path
    post rules_withdrawals_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    get new_rules_withdrawals_pick_asset_path
    assert_response :success

    assert_match(/class="ticker active"/, response.body,
                 'asset picker should wrap the search form in class="ticker active"')
    assert_no_match(/conversational__input sinput/, response.body,
                    'asset picker should no longer carry .conversational__input.sinput on the input')
    assert_match(/#{@binance.name}/, response.body,
                 'asset picker should show the exchange already picked')
  end

  # The asset step returns through the key step, which then looks the destination up for it.
  test 'picking the asset returns through the API key step' do
    get new_rules_withdrawals_pick_exchange_path
    post rules_withdrawals_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: @binance.id } }
    post rules_withdrawals_pick_asset_path,
         params: { bots_dca_single_asset: { asset_id: @bitcoin.id } }
    assert_redirected_to new_rules_withdrawals_add_api_key_path
  end

  # ---- SVG coverage — load-bearing assumption of the grid partial ----

  test 'every withdrawal-capable exchange class has a logo SVG partial' do
    # The grid partial calls `render "svg/exchanges/#{exchange.name_id}"`,
    # which resolves to app/views/svg/exchanges/_<name_id>.html.{erb,haml}.
    # If any withdrawal-capable exchange class is added without its SVG,
    # this test must fail at CI time — NOT just for whichever subclass
    # happens to have a DB row in this transactional test (an earlier
    # version of this test iterated `Exchange.where(...)` and effectively
    # only checked Binance). Iterate the loaded STI subclasses,
    # instantiate each (no save) so we can call `supports_withdrawal?`,
    # and check the partial for every class that returns true.
    Rails.application.eager_load! # ensure every Exchanges::* file is loaded
    withdrawal_capable_classes =
      Exchange.descendants.select { |klass| klass.new.supports_withdrawal? }
    assert_operator withdrawal_capable_classes.size, :>, 0,
                    'no withdrawal-capable exchange classes were discovered — ' \
                    'eager_load! likely failed to register Exchange.descendants'

    lookup_context = ApplicationController.new.lookup_context
    withdrawal_capable_classes.each do |klass|
      name_id = klass.new.name_id
      assert lookup_context.exists?("exchanges/#{name_id}", ['svg'], true),
             "Missing partial app/views/svg/exchanges/_#{name_id}.html.* " \
             "(class #{klass.name})"
    end
  end
end
