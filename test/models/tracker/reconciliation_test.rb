require 'test_helper'

# Closing a finding by writing the entry that is missing.
#
# The quantity is not a guess: it is forced by arithmetic. A history whose running balance reaches
# minus six litecoin is missing at least six litecoin of acquisitions, and they must predate the
# point it got there. The one thing nobody can compute is what those coins COST — so that is the
# only thing the user is asked, and nothing is written until they say so.
#
# What is written is marked as theirs. A re-sync must never overwrite it, dedup must never collide
# with it, and the tax report must be able to tell a stated cost from an estimated one.
class Tracker::ReconciliationTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @btc = create(:asset, :bitcoin)
    @day = ->(n) { Time.utc(2026, 1, n, 12) }
  end

  def tx(type, day:, **attrs)
    defaults = { api_key: @key, exchange: @binance, entry_type: type, transacted_at: @day.call(day) }
    defaults.merge!(quote_currency: nil, quote_amount: nil) if %i[deposit withdrawal].include?(type)
    create(:account_transaction, **defaults, **attrs)
  end

  def overdrawn_account
    # Sold 5 that never arrived, then bought 1: the history is short by 5.
    tx(:sell, day: 3, base_currency: 'BTC', base_amount: 5, quote_currency: 'USD', quote_amount: 50_000)
    tx(:buy, day: 4, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
  end

  def propose = Tracker::Reconciliation.propose(@user, 'BTC')

  # ── what it works out on its own ─────────────────────────────────────────────────────────────
  test 'it computes the quantity the history is short by' do
    overdrawn_account

    proposal = propose

    assert_equal :acquisition, proposal.kind
    assert_equal 5.to_d, proposal.quantity
  end

  # An opening balance is what was already held when the record begins, so it is dated there.
  test 'it dates the entry before the earliest thing we know about' do
    overdrawn_account

    assert_operator propose.on, :<, @day.call(3)
  end

  test 'it offers the market price of that day, from our own prices' do
    overdrawn_account
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: @day.call(3).to_date, price: 20_000)

    proposal = propose

    assert_equal 20_000.to_d, proposal.market_price
    assert_equal 100_000.to_d, proposal.market_cost
  end

  test 'a coin the venue no longer reports needs a disposal, and no price' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    AccountBalance.create!(user: @user, exchange: @binance, asset: @btc, free: 0.25, locked: 0,
                           usd_price: 20_000, usd_value: 5_000, synced_at: Time.current, priced_at: Time.current)

    proposal = propose

    assert_equal :disposal, proposal.kind
    assert_equal 0.75.to_d, proposal.quantity
    assert_nil proposal.market_price
  end

  test 'an account with nothing wrong proposes nothing' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    AccountBalance.create!(user: @user, exchange: @binance, asset: @btc, free: 1, locked: 0,
                           usd_price: 20_000, usd_value: 20_000, synced_at: Time.current, priced_at: Time.current)

    assert_nil propose
  end

  # ── how the coins arrived ────────────────────────────────────────────────────────────────────
  #
  # "What did you pay for it?" is the wrong question for a coin nobody buys. Binance hands out BNB
  # as commission rebates and as the receipt for sweeping dust, so a missing BNB balance accrued —
  # it was never purchased. That is not a wording difference: earned coins are INCOME at the price
  # of the day they arrived, which is both their cost basis and, in most places, a taxable event.
  test 'it reads how this coin has arrived before and proposes the same' do
    tx(:other_income, day: 1, base_currency: 'BTC', base_amount: 0.4)
    tx(:swap_in, day: 2, base_currency: 'BTC', base_amount: 0.4)
    tx(:sell, day: 3, base_currency: 'BTC', base_amount: 5, quote_currency: 'USD', quote_amount: 50_000)

    assert_equal :earned, propose.likely_arrival
  end

  test 'a coin that has only ever been bought proposes a purchase' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    tx(:sell, day: 3, base_currency: 'BTC', base_amount: 5, quote_currency: 'USD', quote_amount: 50_000)

    assert_equal :bought, propose.likely_arrival
  end

  test 'accepting it as earned records income at the price of the day, not a purchase' do
    overdrawn_account
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: @day.call(3).to_date, price: 20_000)

    Tracker::Reconciliation.accept!(@user, 'BTC', arrival: :earned)

    entry = AccountTransaction.for_user(@user).order(:transacted_at).first
    assert_equal 'other_income', entry.entry_type, 'it accrued; nobody paid for it'
    assert_equal 5.to_d, entry.base_amount
    assert_nil entry.quote_amount, 'income is priced at the market, not at a price someone paid'
    assert_equal 'earned', entry.raw_data['arrival']
  end

  test 'earned coins still close the gap' do
    overdrawn_account
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: @day.call(3).to_date, price: 20_000)

    Tracker::Reconciliation.accept!(@user, 'BTC', arrival: :earned)

    assert_empty Tracker::Ledger.for(@user).overdrawn
  end

  # ── saying what this will NOT settle ─────────────────────────────────────────────────────────
  #
  # The quantity proposed is the smallest that stops the running balance going below zero. That
  # closes the impossibility, but it does not promise the END will match what the venue reports —
  # and where it does not, a second finding appears for the same coin the moment the first is
  # settled. Told afterwards, that reads as "nothing happened". So it is said beforehand.
  test 'it says what will still be unresolved after the entry' do
    tx(:sell, day: 3, base_currency: 'BTC', base_amount: 5, quote_currency: 'USD', quote_amount: 50_000)
    tx(:buy, day: 4, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    AccountBalance.create!(user: @user, exchange: @binance, asset: @btc, free: 0.25, locked: 0,
                           usd_price: 20_000, usd_value: 5_000, synced_at: Time.current, priced_at: Time.current)

    # Adding 5 lifts the whole curve by 5, so the account ends holding 1 — but the venue says 0.25.
    assert_equal 0.75.to_d, propose.residual
  end

  test 'nothing left over means nothing to warn about' do
    tx(:sell, day: 3, base_currency: 'BTC', base_amount: 5, quote_currency: 'USD', quote_amount: 50_000)
    tx(:buy, day: 4, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    AccountBalance.create!(user: @user, exchange: @binance, asset: @btc, free: 1,
                           locked: 0, usd_price: 20_000, usd_value: 20_000,
                           synced_at: Time.current, priced_at: Time.current)

    assert_nil propose.residual
  end

  # ── what it writes, once accepted ────────────────────────────────────────────────────────────
  test 'accepting a stated cost writes it, and the history stops being short' do
    overdrawn_account

    Tracker::Reconciliation.accept!(@user, 'BTC', arrival: :bought, cost: 60_000)

    entry = AccountTransaction.for_user(@user).order(:transacted_at).first
    assert_equal 'deposit', entry.entry_type
    assert_equal 5.to_d, entry.base_amount
    assert_equal 60_000.to_d, entry.quote_amount
    assert_empty Tracker::Ledger.for(@user).overdrawn
  end

  test 'accepting without a cost writes the quantity alone' do
    overdrawn_account

    Tracker::Reconciliation.accept!(@user, 'BTC', arrival: :bought, cost: nil)

    entry = AccountTransaction.for_user(@user).order(:transacted_at).first
    assert_equal 5.to_d, entry.base_amount
    assert_nil entry.quote_amount, 'the quantity reconciles; the cost stays unknown'
  end

  # It is the user's entry, not the exchange's — three consequences, all load-bearing.
  test 'what it writes is marked as the user\'s own' do
    overdrawn_account

    Tracker::Reconciliation.accept!(@user, 'BTC', arrival: :bought, cost: 60_000)
    entry = AccountTransaction.for_user(@user).order(:transacted_at).first

    assert_match(/\Amanual-/, entry.tx_id, 'a sync can never collide with it')
    assert_equal 'manual', entry.raw_data['source']
    assert_equal 'stated', entry.raw_data['basis'], 'a stated cost is not an estimated one'
  end

  test 'a cost taken from our prices is recorded as an estimate, not a fact' do
    overdrawn_account
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: @day.call(3).to_date, price: 20_000)

    Tracker::Reconciliation.accept!(@user, 'BTC', arrival: :bought, cost: propose.market_cost, estimated: true)

    assert_equal 'estimated', AccountTransaction.for_user(@user).order(:transacted_at).first.raw_data['basis']
  end

  test 'a disposal closes the disagreement without inventing proceeds' do
    tx(:buy, day: 1, base_currency: 'BTC', base_amount: 1, quote_currency: 'USD', quote_amount: 20_000)
    AccountBalance.create!(user: @user, exchange: @binance, asset: @btc, free: 0.25, locked: 0,
                           usd_price: 20_000, usd_value: 5_000, synced_at: Time.current, priced_at: Time.current)

    Tracker::Reconciliation.accept!(@user, 'BTC', arrival: :bought, cost: nil)

    entry = AccountTransaction.for_user(@user).order(:transacted_at).last
    assert_equal 'withdrawal', entry.entry_type, 'the coins left; nothing was received for them'
    assert_equal 0.to_d, Tracker::Ledger.for(@user).realised_pnl_usd
  end

  # Nothing is written by looking at the page.
  test 'proposing writes nothing' do
    overdrawn_account

    assert_no_difference('AccountTransaction.count') { propose }
  end

  # Priced as the coin this VENUE means by the symbol.
  test 'the proposal prices the coin the venue means' do
    create(:asset, symbol: 'LIT', name: 'Lighter', external_id: 'lighter', category: 'Cryptocurrency', market_cap_rank: 77)
    tx(:sell, day: 2, base_currency: 'LIT', base_amount: 5, quote_currency: 'USDT', quote_amount: 10)
    fetched = []
    MarketData.stubs(:get_historical_price_range).with do |args|
      fetched << args[:coin_id]
      true
    end.returns(Result::Success.new('prices' => []))

    Tracker::Reconciliation.propose(@user, 'LIT')

    assert_includes fetched, 'litentry'
    assert_not_includes fetched, 'lighter'
  end
end
