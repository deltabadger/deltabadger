# frozen_string_literal: true

require 'test_helper'

# Every normalized figure in the app is computed and cached in USD. Denomination is the one
# place a dollar figure becomes something else, so these cover its whole surface: the rate it
# asks for, the fallback when there isn't one, and where the symbol sits.
class DenominationTest < ActiveSupport::TestCase
  test 'USD is the identity and never asks for a rate' do
    Utilities::Currency.expects(:exchange_rate).never

    denomination = Denomination.for('USD')

    assert_equal 'USD', denomination.currency
    assert_equal '<small>$</small>25.00', denomination.format(25)
  end

  test 'no preference at all is USD' do
    Utilities::Currency.expects(:exchange_rate).never

    assert_equal 'USD', Denomination.for(nil).currency
  end

  test 'another currency converts at its USD rate' do
    stub_rate('PLN', 3.8)

    assert_equal '95.00 <small>zł</small>', Denomination.for('PLN').format(25)
  end

  test 'the symbol trails the amount where the currency is written that way' do
    stub_rate('CHF', 0.8)
    stub_rate('EUR', 0.9)

    assert_equal '20.00 <small>CHF</small>', Denomination.for('CHF').format(25)
    assert_equal '<small>€</small>22.50', Denomination.for('EUR').format(25)
  end

  test 'a loss keeps its minus sign in either layout' do
    stub_rate('PLN', 4)

    assert_equal '-40.00 <small>zł</small>', Denomination.for('PLN').format(-10)
    assert_equal '-<small>$</small>10.00', Denomination.for('USD').format(-10)
  end

  # A złoty sign on a dollar figure would be a lie, and the dollar figure is the one we have.
  test 'an unreachable rate falls back to USD rather than mislabelling dollars' do
    Utilities::Currency.expects(:exchange_rate).with(from: 'USD', to: 'PLN')
                       .returns(Result::Failure.new('no feed'))

    denomination = Denomination.for('PLN')

    assert_equal 'USD', denomination.currency
    assert_equal '<small>$</small>25.00', denomination.format(25)
  end

  # The callers hand it whatever they have, including the "rate not cached yet" nil.
  test 'no figure formats to nothing' do
    assert_nil Denomination.for('USD').format(nil)
    assert_nil Denomination.for('USD').format_plain(nil)
  end

  # The tag has to survive ERB, or every tile on the page shows the markup as words.
  test 'the figure is rendered, not escaped' do
    assert_predicate Denomination.for('USD').format(25), :html_safe?
  end

  # A label and a sentence are read, not drawn: a tag there is spelled out.
  test 'the plain form carries no markup' do
    stub_rate('PLN', 3.8)

    assert_equal '$25.00', Denomination.for('USD').format_plain(25)
    assert_equal '95.00 zł', Denomination.for('PLN').format_plain(25)
  end

  # Utilities::Currency caches answers, never failures, so without a backoff of its own every
  # page load during a market-data outage would pay for the same timeout again. The test env
  # runs on :null_store, so the backoff is only observable against a real cache.
  test 'a failed lookup is not retried on the next page load' do
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    Utilities::Currency.expects(:exchange_rate).with(from: 'USD', to: 'PLN')
                       .once.returns(Result::Failure.new('no feed'))

    3.times { assert_equal 'USD', Denomination.for('PLN').currency }
  end

  private

  def stub_rate(currency, rate)
    Utilities::Currency.stubs(:exchange_rate).with(from: 'USD', to: currency)
                       .returns(Result::Success.new(rate))
  end
end
