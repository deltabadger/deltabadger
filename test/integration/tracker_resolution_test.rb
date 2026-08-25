require 'test_helper'

# What the page says when the history and the exchange do not agree: a gray note under the holding
# it concerns, in plain words, naming the exchange and both quantities — and nothing to click.
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

  test 'a resolved holding says what was assumed, in gray, and offers nothing to click' do
    tx(:deposit, base_currency: 'USDC', base_amount: 1_000, quote_currency: nil, quote_amount: nil)
    tx(:buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 0.9, 1_350)
    Tracker::Ledger.compute!(@user)

    get tracker_path

    assert_select '.tracker-holdings__note', text: /Binance.*0\.9.*1\.00/m
    assert_select '.tracker-holdings__pl', text: '+50.0%', count: 1
    assert_select '.tracker-findings', false
    assert_select "a[href*='reconciliation']", false
    assert_select '.data-grid__item__value', text: /\$900\.00/, count: 1
  end

  test 'a coin the exchange no longer holds is noted below the holdings, not listed as held' do
    tx(:deposit, base_currency: 'USDC', base_amount: 1_000, quote_currency: nil, quote_amount: nil)
    tx(:buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 600)
    usdc = create(:asset, symbol: 'USDC', name: 'USD Coin')
    balance(usdc, 400, 400)
    Tracker::Ledger.compute!(@user)

    get tracker_path

    assert_select '.tracker-holdings__row', count: 1
    assert_select '.tracker-holdings__note', text: /Binance.*BTC.*1\.00/m
    assert_select '.tracker-positions tbody tr', { text: /BTC/, count: 0 }, 'left at cost: not an open position'
  end

  test 'with balances hidden the note keeps its words and loses its money' do
    tx(:deposit, base_currency: 'USDC', base_amount: 1_000, quote_currency: nil, quote_amount: nil)
    tx(:buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 0.9, 1_350)
    Tracker::Ledger.compute!(@user)
    @user.update!(hide_balances: true)

    get tracker_path

    assert_select '.tracker-holdings__note', text: /Binance/
    assert_no_match(/\$100\.00|\$900\.00|1,350/, response.body)
  end

  test 'an empty sync is a sync: a coin bought since it is shown at cost, and nothing says never synced' do
    @key.update!(balances_synced_at: @t - 1.day)
    tx(:buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    Tracker::Ledger.compute!(@user)

    get tracker_path

    assert_select '.tracker-holdings__empty', false
    assert_select '.tracker-holdings__row', count: 1
    assert_select '.tracker-holdings__note', text: /BTC.*since the last sync/m
  end
end
