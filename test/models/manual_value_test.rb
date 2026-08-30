require 'test_helper'

# A price the user states, standing in front of everything the app worked out for itself.
#
# The tracker prices what a venue did not report — a dust rebate, an airdrop, a deposit, a
# withdrawal — from our own price history. That is the right default and it covers thousands of
# rows nobody would ever want to type. But some rows cannot be priced at all: a coin with no asset
# record, or one whose ticker resolves to something else entirely. Those are where the figures go
# silent.
#
# What is stated is a PRICE, per unit, in USD — never a value. The record holds amounts and prices;
# a value is worked out from them, later, in whatever currency the reader asks for. A stated price
# wins, and everything downstream reads it through the ONE place every figure already passes:
# `PriceService#enrich`. Tiles, chart, positions and the tax report inherit it without knowing it
# exists.
class ManualValueTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @at = Time.utc(2026, 2, 2, 12)
  end

  def reward(**attrs)
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :other_income,
                                 base_currency: 'ZZZ', base_amount: 10, quote_currency: nil,
                                 quote_amount: nil, transacted_at: @at, **attrs)
  end

  def enriched(record) = Tax::PriceService.new.enrich([record], currency: 'USD').sole

  test 'a row nobody can price says so' do
    row = enriched(reward)

    assert row[:price_missing]
    assert_equal 0.to_d, row[:fiat_value]
  end

  test 'a stated price is used, times the amount, and the row stops being unpriced' do
    record = reward
    record.set_manual(:price, 4.25)
    record.save!

    row = enriched(record.reload)

    assert_equal 42.5.to_d, row[:fiat_value], '10 units at 4.25'
    assert_not row[:price_missing], 'it is known now — the user knows it'
  end

  # The point of the placeholder: our price is the default, not a decision the user has to make.
  test 'a stated price beats a price we could have found ourselves' do
    HistoricalPrice.create!(asset: 'ZZZ', currency: 'USD', date: @at.to_date, price: 3)
    record = reward
    assert_equal 30.to_d, enriched(record)[:fiat_value], 'ours, by default'

    record.set_manual(:price, 9.9)
    record.save!

    assert_equal 99.to_d, enriched(record.reload)[:fiat_value], 'theirs, once stated'
  end

  # The venue's own amount and price ARE the value. A figure typed in front of them would leave
  # amount, price and value on one row that no longer multiply out — the one thing the column
  # promises — so a row the venue valued takes no stated price: not written, and not read if one
  # was written before this rule.
  test 'a row the venue valued is not the user\'s to state' do
    record = create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :buy,
                                          base_currency: 'BTC', base_amount: 1, quote_currency: 'USD',
                                          quote_amount: 20_000, transacted_at: @at)

    assert_raises(ArgumentError) { record.set_manual(:price, 21_000) }

    record.update_column(:manual_values, { 'price' => '21000' }) # written before the rule
    row = enriched(record.reload)
    assert_equal 20_000.to_d, row[:fiat_value]
    assert_not row[:stated_value]
  end

  test 'clearing it hands the row back to our own price' do
    HistoricalPrice.create!(asset: 'ZZZ', currency: 'USD', date: @at.to_date, price: 3)
    record = reward
    record.set_manual(:price, 99)
    record.save!

    record.set_manual(:price, nil)
    record.save!

    assert_not record.reload.manual?(:price)
    assert_equal 30.to_d, enriched(record.reload)[:fiat_value]
  end

  test 'a price that is not a number is not a price' do
    record = reward
    record.set_manual(:price, 'soon')
    record.save!

    assert_not record.reload.manual?(:price)
  end

  # A value is not a field: it is amount times price, worked out later. Nothing stores one.
  test 'only a price can be stated — never a value, never an amount' do
    record = reward

    assert_raises(ArgumentError) { record.set_manual(:fiat_value, 42.5) }
    assert_raises(ArgumentError) { record.set_manual(:base_amount, 5) }
  end

  # A value typed before this rule is restated as the price it implied, so the row reads the same
  # after the change as before it — 42.5 for 10 units was 4.25 a unit, and still is.
  test 'a value stated before the rule is carried over as a price' do
    require Rails.root.join('db/migrate/20260826140000_restate_manual_values_as_prices.rb').to_s
    record = reward
    record.update_column(:manual_values, { 'fiat_value' => '42.5' })

    RestateManualValuesAsPrices.new.up

    assert_equal({ 'price' => '4.25' }, record.reload.manual_values)
    assert_equal 42.5.to_d, enriched(record)[:fiat_value]
  end

  # A split is booked as one SIGNED net delta, and a reverse split is negative. A value stated on
  # such a row was stated on its size, not its sign — the price it implied is value over |amount|,
  # and it survives the change rather than being dropped as unconvertible.
  test 'a value stated on a signed adjustment is carried over on its size' do
    require Rails.root.join('db/migrate/20260826140000_restate_manual_values_as_prices.rb').to_s
    record = reward(entry_type: :adjustment, base_amount: -4)
    record.update_column(:manual_values, { 'fiat_value' => '10' })

    RestateManualValuesAsPrices.new.up

    assert_equal({ 'price' => '2.5' }, record.reload.manual_values)
  end

  # Rolled back with the release that reads only `fiat_value`, a stated price would vanish from the
  # tracker, the ledger and the tax report. Down puts the value back under its old key.
  test 'rolling back restates the price as the value it implied' do
    require Rails.root.join('db/migrate/20260826140000_restate_manual_values_as_prices.rb').to_s
    record = reward
    record.set_manual(:price, 4.25)
    record.save!

    RestateManualValuesAsPrices.new.down

    assert_equal({ 'fiat_value' => '42.5' }, record.reload.manual_values)
  end

  # The rows the retired "Fix" modal wrote were assumptions dated the moment they were accepted —
  # never the venue's, and now a second copy of what the page reads from the balance itself. They
  # go; a real withdrawal with the venue's own id stays, and a link drawn to a retired row is undrawn.
  test 'what the retired modal wrote is removed, and the venue\'s own rows stay' do
    require Rails.root.join('db/migrate/20260826140000_restate_manual_values_as_prices.rb').to_s
    retired = create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :deposit,
                                           base_currency: 'LTC', base_amount: 1, transacted_at: @at + 1.hour,
                                           tx_id: 'manual-bf30cf9be8b7fb0e',
                                           raw_data: { 'source' => 'manual', 'arrival' => 'bought' })
    real = create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :withdrawal,
                                        base_currency: 'LTC', base_amount: 1, transacted_at: @at,
                                        tx_id: 'a1b2c3', linked_transaction: retired)

    Tracker::LedgerJob.expects(:perform_later).with(@user.id)
    PortfolioSnapshot::BackfillJob.expects(:perform_later).with(@user.id)

    RestateManualValuesAsPrices.new.up

    assert_not AccountTransaction.exists?(retired.id)
    assert_nil real.reload.linked_transaction_id
  end
end
