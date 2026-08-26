require 'test_helper'

# The Price column, and where its figure comes from — and the Value column, which is only ever
# worked out from it. Amount, Price and Fee are the RECORD; Value is amount times price, in the
# reader's own currency. A tracker can only ever be as good as the rows under it, and some of those
# rows arrive without a price: a dust rebate, an airdrop, a coin-for-coin swap, a withdrawal.
#
# The Price cell says whose figure it is. The exchange's stands plainly, in the venue's own currency.
# Ours stands as a PLACEHOLDER — visible, used in the figures, and replaceable — in USD, the unit the
# figures are built in. A typed price stands in front of both and is marked as the user's. And a row
# nobody can price is an empty box, which is the honest reading. Value follows, and is never typed.
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

  def row(record, kind)
    get tracker_path
    css_select("##{ActionView::RecordIdentifier.dom_id(record)} .tracker-row__#{kind}").first
  end

  def price_cell(record) = row(record, :price)
  def value_cell(record) = row(record, :value)

  # `form_with` writes a hidden _method field first — the typeable box is the number one.
  def input_for(record) = price_cell(record).css('input[type="number"]').first

  def unquoted(entry_type, symbol, amount, **attrs)
    create(:account_transaction, user: @user, api_key: @key, exchange: @binance, entry_type: entry_type,
                                 base_currency: symbol, base_amount: amount, quote_currency: nil,
                                 quote_amount: nil, transacted_at: @t, **attrs)
  end

  # Priced by the venue: amount and price of its own, and the value is their product. Nothing here
  # is a guess, so there is nothing for a user to state — a box would only let the three columns
  # stop multiplying out.
  test 'a price the exchange reported stands as the exchange&apos;s, and is not a box' do
    bought = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                          entry_type: :buy, base_currency: 'BTC', base_amount: 1,
                                          quote_currency: 'USD', quote_amount: 20_000, transacted_at: @t)

    cell = price_cell(bought)

    assert_includes cell['class'], 'tracker-row__price--exchange'
    assert_empty cell.css('input'), 'nothing here for a user to state'
    assert_equal '20,000.00 USD', cell.text.strip
    assert_equal '20,000.00', value_cell(bought).text.strip
  end

  # A Convert into cash says what the coins fetched as plainly as a quote would: the cash leg beside
  # the coin leg is its price and its value, and there is nothing here for a user to state either.
  test 'a convert into cash is the venue&apos;s figure, and not a box' do
    swapped_out = unquoted(:swap_out, 'BNB', 0.75, group_id: 'convert-1')
    unquoted(:swap_in, 'USDC', 450, group_id: 'convert-1')

    cell = price_cell(swapped_out)

    assert_includes cell['class'], 'tracker-row__price--exchange'
    assert_empty cell.css('input'), 'nothing here for a user to state'
    assert_equal '600.00 USDC', cell.text.strip, 'what one BNB fetched, in what it fetched'
    assert_equal '450.00', value_cell(swapped_out).text.strip

    patch price_tracker_transaction_path(id: swapped_out.id), params: { price: '650' }
    assert_response :unprocessable_entity
  end

  test 'a price the exchange reported is not the user&apos;s to state' do
    bought = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                          entry_type: :buy, base_currency: 'BTC', base_amount: 1,
                                          quote_currency: 'USD', quote_amount: 20_000, transacted_at: @t)

    patch price_tracker_transaction_path(id: bought.id), params: { price: '21000' }

    assert_response :unprocessable_entity
    assert_not bought.reload.manual?(:price)
  end

  # The case that made this necessary. The venue said nothing, so the price is ours — and it is
  # shown as the default it is, in the box, rather than hidden behind an em dash; the value is
  # worked out from it, and is not a box.
  test 'our own price shows as the placeholder when the exchange gave none, and the value follows' do
    HistoricalPrice.store(asset: 'BTC', currency: 'USD', date: @t.to_date, price: 30_000)
    rebate = unquoted(:other_income, 'BTC', 0.5)

    cell = price_cell(rebate)

    assert_includes cell['class'], 'tracker-row__price--ours'
    assert_equal '30000.0', input_for(rebate)['placeholder'], 'our price for that day, per unit'
    assert_nil input_for(rebate)['value'], 'a default is not a statement'
    assert_includes cell.text, 'USD', 'the page is shown in dollars'
    assert_select "##{ActionView::RecordIdentifier.dom_id(rebate)} .tracker-row__info", false, 'priced: nothing to point out'
    assert_equal '15,000.00', value_cell(rebate).text.strip, 'half a coin at 30,000'
    assert_empty value_cell(rebate).css('input'), 'a value is worked out, never typed'
  end

  # A withdrawal is a row like any other: the venue never prices one, so ours is the placeholder.
  test 'a withdrawal gets our price for its day' do
    HistoricalPrice.store(asset: 'BTC', currency: 'USD', date: @t.to_date, price: 30_000)
    withdrawn = unquoted(:withdrawal, 'BTC', 0.2)

    assert_equal '30000.0', input_for(withdrawn)['placeholder']
    assert_equal '6,000.00', value_cell(withdrawn).text.strip
  end

  # A reverse split is one signed, negative delta. It is a row like any other: our price for its
  # day is the placeholder, and its value carries the sign — the shares that left, at that price.
  test 'a signed adjustment is priced, and its value keeps the sign' do
    HistoricalPrice.store(asset: 'BTC', currency: 'USD', date: @t.to_date, price: 30_000)
    reverse_split = unquoted(:adjustment, 'BTC', -0.5)

    assert_includes price_cell(reverse_split)['class'], 'tracker-row__price--ours'
    assert_equal '30000.0', input_for(reverse_split)['placeholder']
    assert_equal '-15,000.00', value_cell(reverse_split).text.strip
  end

  # Nothing to say, and the row says nothing — rather than a figure the page cannot stand behind.
  # The problem is the empty box, so the explanation sits beside it, behind an info mark: this is
  # the one place the reader can do something about it.
  test 'a row nobody can price is an empty box, with the explanation beside it' do
    unpriceable = unquoted(:airdrop, 'WEIRD', 3)

    cell = price_cell(unpriceable)
    assert_includes cell['class'], 'tracker-row__price--none'
    assert_equal '—', input_for(unpriceable)['placeholder']
    assert_equal '—', value_cell(unpriceable).text.strip
    assert_equal I18n.t('tracker.price_source.none'), cell.css('.tracker-row__info .tooltip').text.strip
    assert_select '.tracker-holdings__notes', false
  end

  # A third of the table was fiat and stablecoin rows. They have no price to miss — their worth is
  # their own amount — and flagging them buried the rows that genuinely could not be priced. What
  # they DO need is the rate that carries them into the one unit the Value column is written in.
  # The price of money is its rate: what one euro bought that day, in the column's own unit — so
  # amount times price is the value, on this row as on every other, and nothing is typed here.
  test 'a row whose base is money has its rate for the day as its price' do
    FxRate.create!(currency: 'USD', date: @t.to_date, rate: '1.25'.to_d)
    deposited = unquoted(:deposit, 'EUR', 200)
    parked = unquoted(:deposit, 'USDT', 150)

    assert_equal '1.25 USD', price_cell(deposited).text.strip, 'what one euro bought that day'
    assert_empty price_cell(deposited).css('input'), 'money has no price to state'
    assert_equal '250.00', value_cell(deposited).text.strip, '200 EUR on a day the euro bought 1.25 dollars'
    assert_equal '1.00 USD', price_cell(parked).text.strip, 'a stablecoin at par'
    assert_equal '150.00', value_cell(parked).text.strip
  end

  # The defect this replaced: 5.61 EUR was shown as 5.71 EUR, because it went out to dollars at the
  # rate of its own day and came back at today's. A euro row shown in euro is the row.
  test 'a fiat row shown in its own currency comes back as itself' do
    FxRate.create!(currency: 'USD', date: @t.to_date, rate: '1.25'.to_d)
    Denomination.stubs(:for).returns(Denomination.new('EUR', '0.86'.to_d))
    deposited = unquoted(:deposit, 'EUR', '5.61'.to_d)

    assert_equal '5.61', value_cell(deposited).text.strip
  end

  # The rate for that DAY, not today's. A 2021 row restated at this morning's rate is a different
  # number every morning, and none of them is what happened.
  test 'a money row with no rate for its day is unpriced rather than guessed' do
    deposited = unquoted(:deposit, 'EUR', 200)

    assert_equal '—', value_cell(deposited).text.strip
  end

  test 'price is the venue\'s own, in the venue\'s own currency, whatever the page is shown in' do
    Denomination.stubs(:for).returns(Denomination.new('EUR', '0.86'.to_d))
    bought = create(:account_transaction, user: @user, api_key: @key, exchange: @binance,
                                          entry_type: :buy, base_currency: 'BTC', base_amount: 0.5,
                                          quote_currency: 'USDT', quote_amount: 25_000, transacted_at: @t)

    assert_equal '50,000.00 USDT', price_cell(bought).text.strip, 'the record, not a conversion'
    get tracker_path
    assert_select 'th', text: /\A#{I18n.t('tracker.columns.price')}\z/, count: 1
  end

  test 'a typed price is stored, marked as the user&apos;s, shown in the box, and the value follows' do
    rebate = unquoted(:airdrop, 'BTC', 2)

    patch price_tracker_transaction_path(id: rebate.id), params: { price: '1234.5' }

    assert_equal 1234.5.to_d, rebate.reload.manual_value(:price)
    assert rebate.manual?(:price)
    assert_includes price_cell(rebate)['class'], 'tracker-row__price--stated'
    assert_equal '1234.5', input_for(rebate)['value']
    assert_equal '2,469.00', value_cell(rebate).text.strip, 'two coins at 1,234.50'
  end

  # "you can change it later" — including changing it back to ours.
  test 'clearing the box hands the row back to our own price' do
    HistoricalPrice.store(asset: 'BTC', currency: 'USD', date: @t.to_date, price: 30_000)
    rebate = unquoted(:airdrop, 'BTC', 1)
    patch price_tracker_transaction_path(id: rebate.id), params: { price: '1234.5' }

    patch price_tracker_transaction_path(id: rebate.id), params: { price: '' }

    assert_not rebate.reload.manual?(:price)
    assert_includes price_cell(rebate)['class'], 'tracker-row__price--ours'
  end

  # Amount times Price is Value, in ONE currency, on every row: our price and a typed price are
  # shown in the reader's currency, like the Value beside them. The record still holds the price
  # in USD — a figure typed in euro is banked at today's rate, exactly as it is shown at it — and
  # only the venue's own booked price keeps the venue's currency, because that one is the record.
  test 'our price and a stated price are shown, and typed, in the reader&apos;s currency' do
    Denomination.stubs(:for).returns(Denomination.new('EUR', '0.5'.to_d))
    HistoricalPrice.store(asset: 'BTC', currency: 'USD', date: @t.to_date, price: 30_000)
    rebate = unquoted(:airdrop, 'BTC', 0.5)

    assert_equal '15000.0', input_for(rebate)['placeholder'], '30,000 dollars is 15,000 euro'
    assert_includes price_cell(rebate).text, 'EUR'
    assert_equal '7,500.00', value_cell(rebate).text.strip, 'half a coin at 15,000'

    patch price_tracker_transaction_path(id: rebate.id), params: { price: '100' }

    assert_equal 200.to_d, rebate.reload.manual_value(:price), '100 euro at 0.5 is 200 dollars'
    assert_equal '100.0', input_for(rebate)['value'], 'and it reads back as the 100 they typed'
    assert_equal '50.00', value_cell(rebate).text.strip, 'half a coin at 100'
  end

  # The whole point of stating a price: the figures change. Everything priced goes through
  # PriceService, so the tiles, the chart, the positions and the tax report all inherit it at once.
  test 'a stated price is what the figures are then built on' do
    rebate = unquoted(:airdrop, 'BTC', 2)
    rebate.set_manual(:price, 2_000)
    rebate.save!

    assert_equal 4_000.to_d, Tax::PriceService.new.enrich([rebate], currency: 'USD').first[:fiat_value]
  end

  # A stated cost is defensible; a silent one is not. The report names the rows that rest on the
  # user's own figures, the same way it names deposits whose basis it had to assume.
  test 'the tax report discloses which rows rest on a price the user stated' do
    sold = unquoted(:airdrop, 'BTC', 1, transacted_at: Time.utc(2025, 3, 1))
    sold.set_manual(:price, 4_000)
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

    patch price_tracker_transaction_path(id: other.id), params: { price: '9' }

    assert_response :not_found
    assert_not other.reload.manual?(:price)
  end
end
