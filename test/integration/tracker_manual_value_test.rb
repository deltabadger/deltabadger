require 'test_helper'

# The Value column, and where its figure comes from. A tracker can only ever be as good as the rows
# under it, and some of those rows arrive without a price: a dust rebate, an airdrop, a coin-for-coin
# swap. The page used to answer that with an em dash and carry on stating totals built on the gap.
#
# Now the cell says whose figure it is. The exchange's stands plainly. Ours stands as a PLACEHOLDER —
# visible, used in the figures, and replaceable. A typed value stands in front of both and is marked
# as the user's. And a row nobody can price is an empty box, which is the honest reading.
class TrackerManualValueTest < ActionDispatch::IntegrationTest
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    create(:asset, :bitcoin)
    @t = Time.utc(2026, 8, 1, 12)
    sign_in @user
  end

  def row_cell(record)
    get tracker_path
    css_select("##{ActionView::RecordIdentifier.dom_id(record)} .tracker-row__value")
  end

  # `form_with` writes a hidden _method field first — the typeable box is the number one.
  def input_for(record) = row_cell(record).first.css('input[type="number"]').first

  # Priced by the venue: no placeholder, no mark, just the figure.
  test 'a value the exchange reported stands as the exchange&apos;s' do
    bought = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                          entry_type: :buy, base_currency: 'BTC', base_amount: 1,
                                          quote_currency: 'USD', quote_amount: 20_000, transacted_at: @t)

    cell = row_cell(bought).first

    assert_includes cell['class'], 'tracker-row__value--exchange'
    assert_equal '20000.0', cell.css('input[type="number"]').first['placeholder']
  end

  # The case that made this necessary. The venue said nothing, so the figure is ours — and it is
  # shown as the default it is, in the box, rather than hidden behind an em dash.
  test 'our own price shows as the placeholder when the exchange gave none' do
    HistoricalPrice.store(asset: 'BTC', currency: 'USD', date: @t.to_date, price: 30_000)
    rebate = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                          entry_type: :other_income, base_currency: 'BTC',
                                          base_amount: 0.5, quote_currency: nil, quote_amount: nil,
                                          transacted_at: @t)

    cell = row_cell(rebate).first

    assert_includes cell['class'], 'tracker-row__value--ours'
    assert_equal '15000.0', cell.css('input[type="number"]').first['placeholder'], 'our price, times the amount'
    assert_nil cell.css('input[type="number"]').first['value'], 'a default is not a statement'
  end

  # Nothing to say, and the row says nothing — rather than a figure the page cannot stand behind.
  test 'a row nobody can price is an empty box, and marked' do
    unpriceable = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                               entry_type: :airdrop, base_currency: 'WEIRD',
                                               base_amount: 3, quote_currency: nil, quote_amount: nil,
                                               transacted_at: @t)

    cell = row_cell(unpriceable).first

    assert_includes cell['class'], 'tracker-row__value--none'
    assert_equal '—', cell.css('input[type="number"]').first['placeholder']
  end

  # A third of the table was fiat and stablecoin rows. They have no price to miss — their worth is
  # their own amount — and flagging them buried the rows that genuinely could not be priced. What
  # they DO need is the rate that carries them into the one unit the column is written in.
  test 'a row whose base is money states a fact, in the column&apos;s own unit' do
    FxRate.create!(currency: 'USD', date: @t.to_date, rate: '1.25'.to_d)
    deposited = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                             entry_type: :deposit, base_currency: 'EUR',
                                             base_amount: 200, quote_currency: nil, quote_amount: nil,
                                             transacted_at: @t)

    cell = row_cell(deposited).first

    assert_includes cell['class'], 'tracker-row__value--cash'
    assert_empty cell.css('input'), 'nothing here for a user to state'
    assert_equal '250.00', cell.text.strip, '200 EUR on a day the euro bought 1.25 dollars'
  end

  # The defect this replaced: 5.61 EUR was shown as 5.71 EUR, because it went out to dollars at the
  # rate of its own day and came back at today's. A euro row shown in euro is the row.
  test 'a fiat row shown in its own currency comes back as itself' do
    FxRate.create!(currency: 'USD', date: @t.to_date, rate: '1.25'.to_d)
    Denomination.stubs(:for).returns(Denomination.new('EUR', '0.86'.to_d))
    deposited = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                             entry_type: :deposit, base_currency: 'EUR',
                                             base_amount: '5.61'.to_d, quote_currency: nil,
                                             quote_amount: nil, transacted_at: @t)

    assert_equal '5.61', row_cell(deposited).first.text.strip
  end

  # The rate for that DAY, not today's. A 2021 row restated at this morning's rate is a different
  # number every morning, and none of them is what happened.
  test 'a money row with no rate for its day is unpriced rather than guessed' do
    deposited = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                             entry_type: :deposit, base_currency: 'EUR',
                                             base_amount: 200, quote_currency: nil, quote_amount: nil,
                                             transacted_at: @t)

    assert_includes row_cell(deposited).first['class'], 'tracker-row__value--none'
  end

  # Two adjacent money columns that do not multiply out is the same broken promise as two
  # disagreeing totals — so Price is READ OFF the value rather than computed a second way.
  test 'price times amount comes to value, in that same unit' do
    HistoricalPrice.store(asset: 'BTC', currency: 'USD', date: @t.to_date, price: 30_000)
    rebate = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                          entry_type: :airdrop, base_currency: 'BTC', base_amount: 0.5,
                                          quote_currency: nil, quote_amount: nil, transacted_at: @t)

    get tracker_path
    cells = css_select("##{ActionView::RecordIdentifier.dom_id(rebate)} td").map { |td| td.text.strip }

    assert_includes cells, '30,000.00', 'the price, in the column unit'
    assert_equal '15000.0', row_cell(rebate).first.css('input[type="number"]').first['placeholder']
  end

  test 'a typed value is stored, marked as the user&apos;s, and shown in the box' do
    rebate = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                          entry_type: :airdrop, base_currency: 'BTC', base_amount: 1,
                                          quote_currency: nil, quote_amount: nil, transacted_at: @t)

    patch value_tracker_transaction_path(id: rebate.id), params: { value: '1234.5' }

    assert_equal 1234.5.to_d, rebate.reload.manual_value(:fiat_value)
    assert rebate.manual?(:fiat_value)
    cell = row_cell(rebate).first
    assert_includes cell['class'], 'tracker-row__value--stated'
    assert_equal '1234.5', cell.css('input[type="number"]').first['value']
  end

  # "you can change it later" — including changing it back to ours.
  test 'clearing the box hands the row back to our own price' do
    HistoricalPrice.store(asset: 'BTC', currency: 'USD', date: @t.to_date, price: 30_000)
    rebate = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                          entry_type: :airdrop, base_currency: 'BTC', base_amount: 1,
                                          quote_currency: nil, quote_amount: nil, transacted_at: @t)
    patch value_tracker_transaction_path(id: rebate.id), params: { value: '1234.5' }

    patch value_tracker_transaction_path(id: rebate.id), params: { value: '' }

    assert_not rebate.reload.manual?(:fiat_value)
    assert_includes row_cell(rebate).first['class'], 'tracker-row__value--ours'
  end

  # The box is labelled in the user's currency and the ledger counts in USD. A figure typed in euro
  # and banked as dollars would be a quiet 15% error running through every total on the page.
  test 'a value typed in the user&apos;s currency is banked in USD' do
    Denomination.any_instance.stubs(:rate).returns(0.5.to_d)
    @user.update!(display_currency: 'EUR')
    rebate = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                          entry_type: :airdrop, base_currency: 'BTC', base_amount: 1,
                                          quote_currency: nil, quote_amount: nil, transacted_at: @t)

    patch value_tracker_transaction_path(id: rebate.id), params: { value: '100' }

    assert_equal 200.to_d, rebate.reload.manual_value(:fiat_value), '100 EUR at 0.5 is 200 USD'
    assert_equal '100.0', input_for(rebate)['value'], 'and it reads back as the 100 they typed'
  end

  # The whole point of stating a value: the figures change. Everything priced goes through
  # PriceService, so the tiles, the chart, the positions and the tax report all inherit it at once.
  test 'a stated value is what the figures are then built on' do
    rebate = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                          entry_type: :airdrop, base_currency: 'BTC', base_amount: 1,
                                          quote_currency: nil, quote_amount: nil, transacted_at: @t)
    rebate.set_manual(:fiat_value, 4_000)
    rebate.save!

    assert_equal 4_000.to_d, Tax::PriceService.new.enrich([rebate], currency: 'USD').first[:fiat_value]
  end

  # A stated cost is defensible; a silent one is not. The report names the rows that rest on the
  # user's own figures, the same way it names deposits whose basis it had to assume.
  test 'the tax report discloses which rows rest on a value the user stated' do
    sold = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                        entry_type: :airdrop, base_currency: 'BTC', base_amount: 1,
                                        quote_currency: nil, quote_amount: nil,
                                        transacted_at: Time.utc(2025, 3, 1))
    sold.set_manual(:fiat_value, 4_000)
    sold.save!
    # The report is in euro and the stated figure is in USD, so the only lookup left is the FX rate.
    Tax::EcbFxRates.stubs(:rate).returns('0.9'.to_d)

    csv = Tax::Report.new(country: 'DE', year: 2025,
                          transactions: AccountTransaction.for_user(@user)).to_csv

    # The report is written in the jurisdiction's own language, not the session's.
    assert_includes csv, I18n.t('tax_report.warnings.stated_values', locale: :de)
    assert_includes csv, I18n.t('tax_report.warnings.stated_values_hint', locale: :de)
    assert_includes csv, 'BTC 1.0 2025-03-01'
    assert_not_includes csv, 'BTC/USD 2025-03-01', 'a priced row is not a missing price'
    assert_includes csv, '3600.0', 'and 4,000 stated in USD is what the report counts'
  end

  test 'a row belonging to someone else is not editable' do
    other = create(:account_transaction, user: create(:user, setup_completed: true),
                                         exchange: @binance, api_key: create(:api_key, exchange: @binance),
                                         base_currency: 'BTC', base_amount: 1, transacted_at: @t)

    patch value_tracker_transaction_path(id: other.id), params: { value: '9' }

    assert_response :not_found
    assert_not other.reload.manual?(:fiat_value)
  end
end
