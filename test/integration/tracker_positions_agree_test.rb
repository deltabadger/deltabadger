require 'test_helper'

# The two lists on this page must agree about what is held.
#
# They did not. The holdings card walked the BALANCES, so cash appeared in it; the positions table
# walked the LEDGER's positions, and the ledger deliberately keeps no position for cash — it has no
# cost and no gain to report. Both choices are defensible on their own, and together they told the
# reader they held USDC in one list and not in the other, with nothing saying why.
#
# So the table is built from the same holdings the card is, plus what the ledger knows that the
# balances do not: a coin the venue has stopped reporting still has to appear, because that is
# precisely the row a finding is about.
class TrackerPositionsAgreeTest < ActionDispatch::IntegrationTest
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @btc = create(:asset, :bitcoin)
    @usdc = create(:asset, symbol: 'USDC', name: 'USD Coin')
    @t = Time.utc(2026, 5, 1, 12)
    sign_in @user
  end

  def tx(type, **attrs)
    defaults = { api_key: @key, exchange: @binance, entry_type: type, transacted_at: @t }
    defaults.merge!(quote_currency: nil, quote_amount: nil) if %i[deposit withdrawal].include?(type)
    create(:account_transaction, **defaults, **attrs)
  end

  def balance(asset, quantity, value)
    AccountBalance.create!(user: @user, exchange: @binance, asset: asset, free: quantity, locked: 0,
                           usd_price: value / quantity, usd_value: value,
                           synced_at: Time.current, priced_at: Time.current)
  end

  def rows
    get tracker_path
    css_select('.tracker-positions tbody tr').map { |row| row.css('td')[1].text.strip }
  end

  def holdings = css_select('.tracker-holdings__name b').map(&:text)

  test 'every holding on the card is a row in the table' do
    tx(:deposit, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 900)
    balance(@btc, 1, 1_200)
    balance(@usdc, 100, 100)
    Tracker::Ledger.compute!(@user)

    listed = rows

    assert_includes listed, 'BTC'
    assert_includes listed, 'USDC', 'cash is held, so it is listed — the card lists it'
    assert_equal holdings.sort, listed.uniq.sort
  end

  # Cash has no cost and no gain. It is stated as cash rather than dressed up as a position.
  test 'cash says it is cash, and claims no P/L' do
    tx(:deposit, base_currency: 'USDC', base_amount: 1_000)
    balance(@usdc, 1_000, 1_000)
    Tracker::Ledger.compute!(@user)

    get tracker_path

    assert_select '.tracker-positions tr[data-order-type~="cash"]' do
      assert_select 'td', text: /USDC/
      assert_select 'td', text: '—'
    end
  end

  # The mirror: the history holds it, the venue stopped reporting it. It left at cost — not an open
  # position — and with no row to hang on and nothing to do about it, no sentence either.
  test 'a coin the venue no longer reports is not a row, and is not noted' do
    tx(:deposit, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 900)
    balance(@usdc, 100, 100) # no BTC balance at all
    Tracker::Ledger.compute!(@user)

    assert_not_includes rows, 'BTC'
    assert_select '.tracker-holdings__info', false
    assert_select '.tracker-holdings__notes', false
  end

  test 'the filter offers cash only when there is cash to filter' do
    tx(:deposit, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 900)
    balance(@btc, 1, 1_200)
    balance(@usdc, 100, 100)
    Tracker::Ledger.compute!(@user)

    get tracker_path

    assert_select ".tracker-record__pane[data-pane=pos] .segmented__option[data-value='cash']"
  end
end
