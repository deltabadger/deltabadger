require 'test_helper'

class BotHelperTest < ActionView::TestCase
  # Regression: a pending (open, unfilled) limit order has amount_exec == 0.0 (not nil),
  # which broke the old `amount_exec || amount` fallback and rendered "Buying 0.0 X for 0.0 Y".
  # It should now show the requested amounts with open-order wording.
  test 'pending limit buy shows open-order wording with requested (not executed) amounts' do
    order = build(:transaction, side: :buy, status: :submitted, external_status: :open,
                                base: 'CKB', quote: 'USDC',
                                amount: 7107, amount_exec: 0, quote_amount: 10, quote_amount_exec: 0)

    assert_equal 'Open order to buy 7107.0 CKB for 10.0 USDC', transaction_summary(order)
  end

  test 'pending limit sell shows open-order sell wording with requested amounts' do
    order = build(:transaction, side: :sell, status: :submitted, external_status: :open,
                                base: 'CKB', quote: 'USDC',
                                amount: 7107, amount_exec: 0, quote_amount: 10, quote_amount_exec: 0)

    assert_equal 'Open order to sell 7107.0 CKB for 10.0 USDC', transaction_summary(order)
  end

  test 'filled buy shows bought wording with executed amounts' do
    order = build(:transaction, side: :buy, status: :submitted, external_status: :closed,
                                base: 'CKB', quote: 'USDC',
                                amount: 7107, amount_exec: 7107, quote_amount: 10, quote_amount_exec: 9.99)

    assert_equal 'Bought 7107.0 CKB for 9.99 USDC', transaction_summary(order)
  end
end
