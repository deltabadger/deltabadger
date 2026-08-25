require 'test_helper'

# Settling a finding, with consent.
#
# The page works out what is missing; the user decides whether it is true. Nothing is written by
# looking at the dialog, the dialog states the whole of what would be written, and what lands is
# marked as theirs — a sync must never overwrite it and the tax report must be able to tell a cost
# they stated from one we read off a price chart.
class TrackerReconciliationTest < ActionDispatch::IntegrationTest
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @btc = create(:asset, :bitcoin)
    @t = Time.utc(2026, 3, 1, 12)
    # Five withdrawn that never arrived: the history is short by five. A withdrawal rather than a
    # sale on purpose — a sale returns cash, and cash the balances do not report would make the
    # whole account genuinely not add up, which the backstop would (rightly) report instead.
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :withdrawal,
                                 base_currency: 'BTC', base_amount: 5, quote_currency: nil,
                                 quote_amount: nil, transacted_at: @t)
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :buy,
                                 base_currency: 'BTC', base_amount: 1, quote_currency: 'USD',
                                 quote_amount: 20_000, transacted_at: @t + 1.day)
    AccountBalance.create!(user: @user, exchange: @binance, asset: @btc, free: 1, locked: 0,
                           usd_price: 30_000, usd_value: 30_000, synced_at: Time.current, priced_at: Time.current)
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: @t.to_date, price: 20_000)
    Tracker::Ledger.compute!(@user)
    sign_in @user
  end

  test 'the page says what it cannot vouch for, and offers to settle it' do
    get tracker_path

    assert_select '.tracker-findings li', text: /BTC/
    assert_select ".tracker-findings a[href*='reconciliation']"
  end

  # The whole of what would be written, before any of it is.
  test 'the dialog states the amount, the date and the cost it would record' do
    get new_tracker_reconciliation_path(symbol: 'BTC')

    assert_response :success
    assert_select '.modal', text: /5/
    # The cost it would record is offered, filled in, and changeable — never assumed. In the
    # currency the label names: a field marked EUR that stores dollars is the same class of quiet
    # inconsistency this whole panel exists to end.
    shown = css_select('input[name=cost]').first['value'].to_d
    assert_equal @user.denomination.convert(100_000).round(2), shown
    assert_select "input[type=submit][value='#{I18n.t('tracker.findings.fix.accept')}']"
  end

  test 'opening the dialog writes nothing' do
    assert_no_difference('AccountTransaction.count') { get new_tracker_reconciliation_path(symbol: 'BTC') }
  end

  # A coin that accrued has no price anyone paid — the dialog asks how it arrived before it asks
  # what it cost, and defaults to what that coin's own history shows.
  test 'a coin that only ever accrued is offered as earned, and recorded as income' do
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :other_income,
                                 base_currency: 'BNB', base_amount: 0.1, quote_currency: nil,
                                 quote_amount: nil, transacted_at: @t)
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :sell,
                                 base_currency: 'BNB', base_amount: 0.4, quote_currency: 'USD',
                                 quote_amount: 200, transacted_at: @t + 1.hour)
    Tracker::Ledger.compute!(@user)

    get new_tracker_reconciliation_path(symbol: 'BNB')
    assert_select ".segmented__option[data-value='earned'].is-on"
    assert_select "input[name=arrival][value='earned']"
    # The price question belongs to a purchase, and the field it holds is disabled while hidden, so
    # an earned entry cannot carry a cost by accident.
    assert_select "[data-select-toggle-value='bought'] input[name=cost]"

    post tracker_reconciliation_path, params: { symbol: 'BNB', arrival: 'earned' }

    entry = AccountTransaction.for_user(@user).order(:transacted_at).first
    assert_equal 'other_income', entry.entry_type
    assert_nil entry.quote_amount, 'nobody paid for it'
    assert_equal 'earned', entry.raw_data['arrival']
  end

  # Said before it is clicked, and again in the message: this entry closes the impossible history,
  # and the leftover it cannot close is a separate gap. Discovering that afterwards is what makes a
  # working fix look like nothing happened.
  test 'it warns up front about what the entry will not settle' do
    AccountBalance.for_user(@user).sole.update!(free: 0.25, usd_value: 7_500)
    Tracker::Ledger.compute!(@user)

    get new_tracker_reconciliation_path(symbol: 'BTC')
    assert_select '.modal .import-note', text: /0\.75/

    post tracker_reconciliation_path, params: { symbol: 'BTC', arrival: 'bought', cost: '', source: 'unknown' }
    assert_match '0.75', flash[:notice].to_s + response.body
  end

  test 'accepting writes it, and the finding clears' do
    typed = @user.denomination.convert(60_000).round(2)

    post tracker_reconciliation_path, params: { symbol: 'BTC', arrival: 'bought', cost: typed.to_s, source: 'own' }

    entry = AccountTransaction.for_user(@user).order(:transacted_at).first
    assert_equal 'deposit', entry.entry_type
    assert_equal 5.to_d, entry.base_amount
    assert_in_delta 60_000, entry.quote_amount.to_f, 1, 'typed in their currency, stored in USD'
    assert_equal 'stated', entry.raw_data['basis']

    Tracker::Ledger.compute!(@user)
    get tracker_path
    assert_select '.tracker-findings', false, 'nothing left in doubt'
  end

  test 'taking our price records it as an estimate, not as a fact' do
    post tracker_reconciliation_path, params: { symbol: 'BTC', arrival: 'bought', cost: '100000.0', source: 'market' }

    assert_equal 'estimated', AccountTransaction.for_user(@user).order(:transacted_at).first.raw_data['basis']
  end

  test 'answering "I do not know" settles the amount and leaves the cost open' do
    post tracker_reconciliation_path, params: { symbol: 'BTC', arrival: 'bought', cost: '', source: 'unknown' }

    entry = AccountTransaction.for_user(@user).order(:transacted_at).first
    assert_equal 5.to_d, entry.base_amount
    assert_nil entry.quote_amount
    assert_equal 'unknown', entry.raw_data['basis']
  end

  # It is the user's entry, and every sync path has to leave it alone.
  test 'what it writes carries no exchange identity a sync could claim' do
    post tracker_reconciliation_path, params: { symbol: 'BTC', arrival: 'bought', cost: '60000', source: 'own' }

    entry = AccountTransaction.for_user(@user).order(:transacted_at).first
    assert_match(/\Amanual-/, entry.tx_id)
    assert_nil entry.api_key_id
    assert_equal 'manual', entry.raw_data['source']
  end

  test 'a symbol with nothing wrong has nothing to settle' do
    get new_tracker_reconciliation_path(symbol: 'ETH')

    assert_redirected_to tracker_path
  end
end
