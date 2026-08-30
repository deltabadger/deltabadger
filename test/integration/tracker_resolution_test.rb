require 'test_helper'

# What the page says when the history and the exchange do not agree — and WHERE. The message sits
# at the end of the row it concerns, behind an info mark, and names the exchange and both quantities
# in plain words: a difference the history does not explain is something missing from the history,
# and the row is where that can be acted on. Nothing is written under the list, and a coin nobody
# holds any more gets no sentence at all: there is no row for it and nothing to do about it.
class TrackerResolutionTest < ActionDispatch::IntegrationTest
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance, balances_synced_at: Time.current)
    @btc = create(:asset, :bitcoin)
    @t = Time.utc(2026, 1, 2, 12)
    sign_in @user
  end

  def tx(type, **attrs)
    create(:account_transaction, user: @user, api_key: @key, exchange: @binance, entry_type: type,
                                 transacted_at: @t, **attrs)
  end

  def balance(asset, quantity, value)
    AccountBalance.create!(user: @user, exchange: @binance, asset: asset, free: quantity, locked: 0,
                           usd_price: value / quantity, usd_value: value, synced_at: Time.current, priced_at: Time.current)
  end

  # A cash row is only on the card when the switch beside the venues asks for it — the default is
  # the invested portfolio. The findings below are about cash, so they ask.
  def show_cash!
    @user.update!(tracker_settings: { 'show_cash' => true })
  end

  test 'a resolved holding says what was assumed at the end of its own row, behind an info mark' do
    tx(:deposit, base_currency: 'USDC', base_amount: 1_000, quote_currency: nil, quote_amount: nil)
    tx(:buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 0.9, 1_350)
    Tracker::Ledger.compute!(@user)

    get tracker_path

    assert_select '.tracker-holdings__row .tracker-holdings__info .tooltip', text: /Binance.*0\.9.*1\.00/m
    assert_select '.tracker-holdings__row .tracker-holdings__info[data-controller="tooltip"] .icon-24', count: 1
    assert_select '.tracker-holdings__note', false, 'nothing under the row'
    assert_select '.tracker-holdings__notes', false, 'nothing under the list'
    assert_select '.tracker-holdings__pl', text: '+50.0%', count: 1
    assert_select '.tracker-findings', false
    assert_select "a[href*='reconciliation']", false
    assert_select '.data-grid__item__value', text: /\$900\.00/, count: 1
  end

  test 'a coin the exchange no longer holds is neither listed nor noted: no row, nothing to do' do
    show_cash!
    tx(:deposit, base_currency: 'USDC', base_amount: 1_000, quote_currency: nil, quote_amount: nil)
    tx(:buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 600)
    usdc = create(:asset, symbol: 'USDC', name: 'USD Coin')
    balance(usdc, 400, 400)
    Tracker::Ledger.compute!(@user)

    get tracker_path

    assert_select '.tracker-holdings__row', count: 1
    assert_select '.tracker-holdings__info', false
    assert_select '.tracker-holdings__notes', false
    assert_no_match(/BTC.*1\.00.*left at cost/m, response.body)
    assert_select '.tracker-positions tbody tr', { text: /BTC/, count: 0 }, 'left at cost: not an open position'
  end

  test 'with balances hidden the note keeps its words and loses its money' do
    tx(:deposit, base_currency: 'USDC', base_amount: 1_000, quote_currency: nil, quote_amount: nil)
    tx(:buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 0.9, 1_350)
    Tracker::Ledger.compute!(@user)
    @user.update!(hide_balances: true)

    get tracker_path

    assert_select '.tracker-holdings__row .tracker-holdings__info .tooltip', text: /Binance/
    assert_no_match(/\$100\.00|\$900\.00|1,350/, response.body)
  end

  test 'an empty sync is a sync: a coin bought since it is shown at cost, and nothing says never synced' do
    @key.update!(balances_synced_at: @t - 1.day)
    tx(:buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    Tracker::Ledger.compute!(@user)

    get tracker_path

    assert_select '.tracker-holdings__empty', false
    assert_select '.tracker-holdings__row', count: 1
    assert_select '.tracker-holdings__row .tracker-holdings__info .tooltip', text: /BTC.*since the last sync/m
  end

  # Cash is a holding like any other: what the history holds and the exchange does not is missing
  # from the history, and the cash row is where that is said — not in a list under everything.
  test 'cash the history holds more of than the exchange gets its note on the cash row' do
    show_cash!
    tx(:deposit, base_currency: 'USDC', base_amount: 1_000, quote_currency: nil, quote_amount: nil)
    tx(:buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 600)
    usdc = create(:asset, symbol: 'USDC', name: 'USD Coin')
    balance(usdc, 300, 300)
    balance(@btc, 1, 900)
    Tracker::Ledger.compute!(@user)

    get tracker_path

    assert_select '.tracker-holdings__row', count: 2
    assert_select '.tracker-holdings__row', text: /USDC/ do
      assert_select '.tracker-holdings__info .tooltip', text: /400.*300.*moved out/m
    end
    assert_select '.tracker-holdings__row', text: /BTC/ do
      assert_select '.tracker-holdings__info', false
    end
    assert_select '.tracker-holdings__notes', false
  end
end
