require 'test_helper'

# The price range is the UTC day, whatever zone the host runs in. `Date#to_time` uses the host's
# zone, so a self-hosted install west of Greenwich asked for a window shifted by its offset.
class Tax::PriceServiceTimezoneTest < ActiveSupport::TestCase
  test 'the requested range is the UTC day on a host outside UTC' do
    from = Date.new(2024, 1, 10)
    to = Date.new(2024, 1, 12)
    args = nil
    MarketData.stubs(:get_historical_price_range).with { |a| args = a }.returns(Result::Failure.new('offline'))

    previous = ENV.fetch('TZ', nil)
    begin
      ENV['TZ'] = 'America/New_York'
      Tax::PriceService.new.fetch_price_range(coin_id: 'bitcoin', symbol: 'BTC', currency: 'USD', from: from, to: to)
    ensure
      ENV['TZ'] = previous
    end

    assert_equal Time.utc(2024, 1, 10).to_i, args[:from].to_i
    assert_equal Time.utc(2024, 1, 13).to_i, args[:to].to_i
  end
end
