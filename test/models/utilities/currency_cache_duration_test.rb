# frozen_string_literal: true

require 'test_helper'

# These rates feed one thing: the USD figures on the dashboard. Holding the slow-moving ones
# for hours is what keeps a EUR or USDT tile from losing its amount between warm passes — but
# a crypto quote's "rate" is a live price, and must not be held that way.
class Utilities::CurrencyCacheDurationTest < ActiveSupport::TestCase
  test 'fiat and stablecoin rates are held for hours' do
    assert_equal Utilities::Currency::STABLE_CACHE_DURATION,
                 Utilities::Currency.cache_duration_for(from: 'EUR', to: 'USD')
    assert_equal Utilities::Currency::STABLE_CACHE_DURATION,
                 Utilities::Currency.cache_duration_for(from: 'usdt', to: 'usd')
  end

  test 'a volatile quote keeps the short window' do
    assert_equal Utilities::Currency::CACHE_DURATION,
                 Utilities::Currency.cache_duration_for(from: 'BTC', to: 'USD')
    assert_equal Utilities::Currency::CACHE_DURATION,
                 Utilities::Currency.cache_duration_for(from: 'USD', to: 'ETH')
  end

  test 'a rate is stored for its own duration' do
    Utilities::Currency.stubs(:calculate_exchange_rate).returns(Result::Success.new(1.1))
    Rails.cache.expects(:write).with('exchange_rate_EUR_to_USD', anything,
                                     expires_in: Utilities::Currency::STABLE_CACHE_DURATION)

    assert_equal 1.1, Utilities::Currency.exchange_rate(from: 'EUR', to: 'USD').data
  end

  # A failed lookup is not an answer. Keeping one would stop every reader retrying for the
  # whole window — twelve hours of a dashboard with no USD figures because one call timed out.
  test 'a failed lookup is never cached' do
    Utilities::Currency.stubs(:calculate_exchange_rate).returns(Result::Failure.new('upstream down'))
    Rails.cache.expects(:write).never

    assert Utilities::Currency.exchange_rate(from: 'EUR', to: 'USD').failure?
  end
end
