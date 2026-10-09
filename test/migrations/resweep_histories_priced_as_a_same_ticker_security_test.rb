require 'test_helper'
require Rails.root.join('db/migrate/20261009120000_resweep_histories_priced_as_a_same_ticker_security.rb')

# Before a broker holding was valued by the asset its rows recorded, a coin at Alpaca was priced as
# any security the catalogue had under its ticker. Those histories are complete and their rows
# unchanged, so nothing would sweep them again on its own.
class ResweepHistoriesPricedAsASameTickerSecurityTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @d0 = Date.new(2026, 1, 1)
    @alpaca = create(:alpaca_exchange)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    @coin = Asset.find_by(external_id: 'bitcoin') || create(:asset, :bitcoin)
    stock = create(:asset, symbol: 'BTC', external_id: 'BTC.US', category: 'Stock', instrument_type: 'etf')
    create(:ticker, exchange: @alpaca, base_asset: stock, quote_asset: usd, ticker: 'BTC', base: 'BTC', quote: 'USD')
    create(:ticker, exchange: @alpaca, base_asset: @coin, quote_asset: usd, ticker: 'BTC/USD', base: 'BTC', quote: 'USD')
    HistoricalPrice.create!(asset: 'stock:BTC', currency: 'USD', date: @d0 + 1, price: 30)
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: @d0 + 1, price: 60_000)
  end

  # A history swept and stamped current, as the old sweep left it: the coin valued at the security's 30.
  def swept_history(user, exchange, base_currency, base_asset: nil)
    create(:account_transaction, api_key: create(:api_key, user: user, exchange: exchange), entry_type: :buy,
                                 base_currency: base_currency, base_asset: base_asset, base_amount: 1, quote_currency: 'USD',
                                 quote_amount: 60_000, transacted_at: (@d0 + 1).to_time(:utc) + 12.hours)
    PortfolioSnapshot.create!(user: user, date: @d0 + 1, value_usd: 30, invested_usd: 60_000, held_value_usd: 30,
                              held_cost_usd: 60_000, partial: false)
    PortfolioSnapshot.mark_history_swept!(user, PortfolioSnapshot.history_version(user))
  end

  test 'a history with a coin under a ticker the catalogue also has as a security is swept again, and comes back at the coin\'s price' do
    holder, crypto_venue, other_symbol = create_list(:user, 3)
    swept_history(holder, @alpaca, 'BTC', base_asset: @coin)
    swept_history(crypto_venue, create(:binance_exchange), 'BTC')
    swept_history(other_symbol, @alpaca, 'ETH')

    ActiveRecord::Migration.suppress_messages { ResweepHistoriesPricedAsASameTickerSecurity.new.up }

    assert PortfolioSnapshot.history_stale?(holder)
    assert_not PortfolioSnapshot.history_stale?(crypto_venue), 'a crypto venue never priced a coin as a security'
    assert_not PortfolioSnapshot.history_stale?(other_symbol), 'no security shares its ticker'

    Exchanges::Alpaca.any_instance.stubs(:set_client)
    Exchanges::Alpaca.any_instance.stubs(:get_candles).returns(Result::Failure.new('offline'))
    MarketData.stubs(:get_historical_price_range).returns(Result::Failure.new('offline'))
    travel_to((@d0 + 2).to_time(:utc) + 12.hours) { PortfolioSnapshot::BackfillJob.perform_now(holder.id) }

    assert_equal 60_000.to_d, PortfolioSnapshot.for_user(holder).sole.held_value_usd
    assert_not PortfolioSnapshot.history_stale?(holder)

    ActiveRecord::Migration.suppress_messages { ResweepHistoriesPricedAsASameTickerSecurity.new.up }
    assert PortfolioSnapshot.history_stale?(holder), 'running it again only asks for one more sweep'
  end
end
