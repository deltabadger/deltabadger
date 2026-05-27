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

  # ---- Task 1: asset picker uses .ticker.active on the search form ----

  test 'pick_asset step renders the search input as a .ticker.active form, not .sinput' do
    get new_rules_withdrawals_pick_asset_path
    assert_response :success
    assert_match(/class="ticker active"/, response.body,
                 'asset picker should wrap the search form in class="ticker active"')
    assert_no_match(/conversational__input sinput/, response.body,
                    'asset picker should no longer carry .conversational__input.sinput on the input')
  end

  # ---- Task 2: exchange picker swaps the list/header for the grid partial ----

  test 'pick_exchange step renders the new exchange grid, not the old fees-header list' do
    # Drive the wizard via real HTTP so session is populated naturally —
    # do not mutate session[] directly in integration tests.
    post rules_withdrawals_pick_asset_path,
         params: { bots_dca_single_asset: { asset_id: @bitcoin.id } }
    follow_redirect!
    assert_response :success

    assert_match(/exchange-grid__item/, response.body,
                 'exchange picker should render the new .exchange-grid__item cards')
    assert_no_match(/exchange-picker__item--header/, response.body,
                    'exchange picker should no longer render the old Maker/Taker header row')
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
