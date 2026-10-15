require 'test_helper'

# script/scan_security_priced_coins.sql: the read-only report a repair starts from.
class ScanSecurityPricedCoinsTest < ActiveSupport::TestCase
  SQL = Rails.root.join('script/scan_security_priced_coins.sql').read

  def statements = SQL.lines.grep_v(/\A\s*--/).join.split(';').map(&:strip).reject(&:empty?)

  def day(n) = Date.new(2026, 10, 1) + n

  test 'it lists the bare coin prices a security wrote, and every user whose history read them' do
    alpaca = create(:alpaca_exchange)
    create(:asset, :bitcoin) unless Asset.exists?(external_id: 'bitcoin')
    create(:asset, symbol: 'BTC', external_id: 'BTC.US', category: 'Stock')
    broker = create(:user)
    create(:account_transaction, api_key: create(:api_key, user: broker, exchange: alpaca), entry_type: :fee, base_currency: 'BTC',
                                 quote_currency: nil, quote_amount: nil, transacted_at: day(7).to_time(:utc))
    crypto_only = create(:user)
    (0..13).each do |n|
      poisoned = (7..11).cover?(n)
      HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: day(n), price: poisoned ? 36 + n : 80_000 + n)
      HistoricalPrice.create!(asset: 'stock:BTC', currency: 'USD', date: day(n), price: 36 + n) if n.between?(6, 9)
    end
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: day(8), price: 33)
    HistoricalPrice.create!(asset: 'BTC', currency: 'EUR', date: day(9), price: 70_000)
    HistoricalPrice.create!(asset: 'ETH', currency: 'USD', date: day(8), price: 1)
    [[broker, 2], [crypto_only, 9], [create(:user), 20]].each do |user, n|
      PortfolioSnapshot.create!(user: user, date: day(n), value_usd: 1, invested_usd: 1, held_value_usd: 1, held_cost_usd: 1, partial: false)
    end
    statements.each { |sql| assert_match(/\AWITH\b/i, sql, 'read-only: queries only') }

    suspects, users = statements.map { |sql| ActiveRecord::Base.connection.select_all(sql).to_a }

    assert_equal [%w[BTC EUR] + [day(8).to_s]] + (7..11).map { |n| ['BTC', 'USD', day(n).to_s] },
                 suspects.map { |row| row.values_at('asset', 'currency', 'date').map(&:to_s) }
    assert_equal [crypto_only.id], users.map { |row| row['user_id'] }, 'a crypto-only user whose day read them'
  end

  test 'a symbol no broker row names is out of scope' do
    create(:asset, :bitcoin) unless Asset.exists?(external_id: 'bitcoin')
    create(:asset, symbol: 'BTC', external_id: 'BTC.US', category: 'Stock')
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: day(0), price: 80_000)
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: day(1), price: 36)

    assert_empty ActiveRecord::Base.connection.select_all(statements.first).to_a
  end
end
