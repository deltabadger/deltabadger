# frozen_string_literal: true

require 'test_helper'

class BotApi::Tracker::SetTransactionPriceTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    api_key = create(:api_key, user: @user)
    # A deposit has no quote leg, so the venue never valued it — it is the row a stated price is for.
    @deposit = create(:account_transaction, :deposit, api_key: api_key, base_amount: 1)
    @buy = create(:account_transaction, api_key: api_key, base_amount: 1, quote_amount: 25_000)
    Tracker::LedgerJob.stubs(:perform_later)
  end

  def price(txn, value) = BotApi::Tracker::SetTransactionPrice.call(user: @user, transaction_id: txn.id, price_usd: value)

  test 'states a price on a row the venue did not value, and asks for the ledger again' do
    Tracker::LedgerJob.expects(:perform_later).with(@user.id)

    result = price(@deposit, '123.45')

    assert result.success?, result.error_message
    assert_equal '123.45', result.data[:price_usd]
    assert_equal BigDecimal('123.45'), @deposit.reload.manual_value(:price)
  end

  test 'a blank price clears the stated one' do
    price(@deposit, '123.45')

    ['', nil].each do |blank|
      @deposit.reload.set_manual(:price, '99')
      @deposit.save!
      assert price(@deposit, blank).success?
      assert_nil @deposit.reload.manual_value(:price)
    end
  end

  test 'a row the venue priced cannot be restated' do
    result = price(@buy, '1')

    assert_equal 'venue_valued', result.error_code
    assert_equal :conflict, result.status
  end

  # parse_manual answers nil for garbage AND for a negative, and set_manual reads nil as "clear",
  # so without a strict check first '-5' would silently delete the stated price.
  test 'garbage and negatives are refusals, not silent clears' do
    price(@deposit, '50')

    ['abc', '-5', '1e2', 'Infinity'].each do |bad|
      result = price(@deposit, bad)
      assert_equal 'invalid_price', result.error_code, bad
      assert_equal BigDecimal('50'), @deposit.reload.manual_value(:price), bad
    end
  end

  test 'an unknown transaction is a 404' do
    assert_equal 'transaction_not_found',
                 BotApi::Tracker::SetTransactionPrice.call(user: @user, transaction_id: 0, price_usd: '1').error_code
  end
end
