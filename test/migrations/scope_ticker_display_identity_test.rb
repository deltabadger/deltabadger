require 'test_helper'
require Rails.root.join('db/migrate/20261006100000_scope_ticker_display_identity.rb')

class ScopeTickerDisplayIdentityTest < ActiveSupport::TestCase
  test 'migration preserves bug tombstones, is idempotent, and keeps both other unique indexes' do
    exchange = create(:alpaca_exchange)
    usd = create(:asset, :usd)
    coin = create(:asset, :bitcoin)
    stock = create(:asset, external_id: 'BTC.US', symbol: 'BTC', category: 'Stock')
    create(:ticker, exchange:, base_asset: coin, quote_asset: usd, ticker: 'BTC/USD', base: 'BTC', quote: 'USD')
    create(:ticker, exchange:, base_asset: stock, quote_asset: usd, ticker: '__stale_2_BTC', base: '__stale_2_BTC', quote: 'USD', available: false)
    c = ActiveRecord::Base.connection
    c.add_index(:tickers, %i[exchange_id base quote], unique: true, name: ScopeTickerDisplayIdentity::OLD)
    before = c.select_all('SELECT * FROM tickers').to_a
    2.times { ScopeTickerDisplayIdentity.new.migrate(:up) }
    assert_equal before, c.select_all('SELECT * FROM tickers').to_a
    refute c.column_exists?(:tickers, :asset_class)
    refute c.index_exists?(:tickers, name: ScopeTickerDisplayIdentity::OLD)
    [%w[exchange_id base_asset_id quote_asset_id], %w[exchange_id ticker]].each do |columns|
      assert(c.indexes(:tickers).any? { |index| index.unique && index.columns == columns })
    end
    assert_raises(ActiveRecord::IrreversibleMigration) { ScopeTickerDisplayIdentity.new.migrate(:down) }
  ensure
    Ticker.reset_column_information
  end
end
