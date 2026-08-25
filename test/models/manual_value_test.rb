require 'test_helper'

# A value the user states, standing in front of everything the app worked out for itself.
#
# The tracker prices what a venue did not report — a dust rebate, an airdrop, a deposit — from our
# own price history. That is the right default and it covers thousands of rows nobody would ever
# want to type. But some rows cannot be priced at all: a coin with no asset record, or one whose
# ticker resolves to something else entirely. Those are where the figures go silent.
#
# So a stated value wins, and everything downstream reads it through the ONE place every figure
# already passes: `PriceService#enrich`. Tiles, chart, positions and the tax report inherit it
# without knowing it exists.
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

  test 'a stated value is used, and the row stops being unpriced' do
    record = reward
    record.set_manual(:fiat_value, 42.5)
    record.save!

    row = enriched(record.reload)

    assert_equal 42.5.to_d, row[:fiat_value]
    assert_not row[:price_missing], 'it is known now — the user knows it'
  end

  # The point of the placeholder: our price is the default, not a decision the user has to make.
  test 'a stated value beats a price we could have found ourselves' do
    HistoricalPrice.create!(asset: 'ZZZ', currency: 'USD', date: @at.to_date, price: 3)
    record = reward
    assert_equal 30.to_d, enriched(record)[:fiat_value], 'ours, by default'

    record.set_manual(:fiat_value, 99)
    record.save!

    assert_equal 99.to_d, enriched(record.reload)[:fiat_value], 'theirs, once stated'
  end

  # It beats even what the venue reported: a user correcting a venue is the whole point of being
  # able to state a value, and the row says plainly that they did.
  test 'a stated value beats the exchange, and the row admits whose figure it is' do
    record = create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :buy,
                                          base_currency: 'BTC', base_amount: 1, quote_currency: 'USD',
                                          quote_amount: 20_000, transacted_at: @at)
    record.set_manual(:fiat_value, 21_000)
    record.save!

    assert_equal 21_000.to_d, enriched(record.reload)[:fiat_value]
    assert record.reload.manual?(:fiat_value)
  end

  test 'clearing it hands the row back to our own price' do
    HistoricalPrice.create!(asset: 'ZZZ', currency: 'USD', date: @at.to_date, price: 3)
    record = reward
    record.set_manual(:fiat_value, 99)
    record.save!

    record.set_manual(:fiat_value, nil)
    record.save!

    assert_not record.reload.manual?(:fiat_value)
    assert_equal 30.to_d, enriched(record.reload)[:fiat_value]
  end

  test 'a value that is not a number is not a value' do
    record = reward
    record.set_manual(:fiat_value, 'soon')
    record.save!

    assert_not record.reload.manual?(:fiat_value)
  end

  test 'only fields we know how to use can be stated' do
    record = reward

    assert_raises(ArgumentError) { record.set_manual(:base_amount, 5) }
  end
end
