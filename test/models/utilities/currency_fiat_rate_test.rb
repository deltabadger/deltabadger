require 'test_helper'

class Utilities::CurrencyFiatRateTest < ActiveSupport::TestCase
  def rate(rates, from, to)
    MarketData.stubs(:get_exchange_rates).returns(Result::Success.new(rates))
    Utilities::Currency.send(:fiat_to_fiat_rate, from, to)
  end

  test 'whole-number rates convert without truncating' do
    result = rate({ 'usd' => { 'value' => 100 }, 'eur' => { 'value' => 80 } }, 'EUR', 'USD')

    assert result.success?
    assert_in_delta 1.25, result.data, 1e-12
    assert_in_delta 0.8, rate({ 'usd' => { 'value' => 100 }, 'eur' => { 'value' => 80 } }, 'USD', 'EUR').data, 1e-12
  end

  test 'float rates convert as before' do
    assert_in_delta 1.25, rate({ 'usd' => { 'value' => 100.0 }, 'eur' => { 'value' => 80.0 } }, 'EUR', 'USD').data, 1e-12
  end

  test 'a zero or missing rate is a failure, never a zero rate' do
    assert rate({ 'usd' => { 'value' => 100 }, 'eur' => { 'value' => 0 } }, 'EUR', 'USD').failure?
    assert rate({ 'usd' => { 'value' => 100 } }, 'EUR', 'USD').failure?
  end
end
