require 'test_helper'

# A split of Alpaca's BTC security, recorded with its asset, never restates a BTC coin holding,
# before or after the security's ticker is restored.
class AlpacaCollisionMatchingPreservedTest < ActiveSupport::TestCase
  test 'restoring a ticker does not change existing account transaction matching' do
    user = create(:user)
    exchange = create(:alpaca_exchange)
    api_key = create(:api_key, user: user, exchange: exchange)
    usd = create(:asset, :usd)
    bitcoin = create(:asset, external_id: 'bitcoin', symbol: 'BTC', category: 'Cryptocurrency')
    stock = create(:asset, external_id: 'BTC.US', symbol: 'BTC', category: 'Stock')
    create(:ticker, exchange: exchange, base_asset: bitcoin, quote_asset: usd,
                    ticker: 'BTC/USD', base: 'BTC', quote: 'USD')
    # This is the bug's stored state: the other row is retained under a tombstone.
    stock_ticker = create(:ticker, exchange: exchange, base_asset: stock, quote_asset: usd,
                                   ticker: '__stale_999_BTC', base: '__stale_999_BTC', quote: 'USD', available: false)
    bot = create(:dca_single_asset, user: user, exchange: exchange,
                                    base_asset: bitcoin, quote_asset: usd, with_api_key: false)
    create(:transaction, bot: bot, exchange: exchange, base: 'BTC', quote: 'USD',
                         base_asset_id: bitcoin.id, quote_asset_id: usd.id,
                         amount: 1, amount_exec: 1, price: 50_000,
                         quote_amount: 50_000, quote_amount_exec: 50_000,
                         created_at: Time.utc(2026, 9, 1), updated_at: Time.utc(2026, 9, 1))
    create(:account_transaction, user: user, api_key: api_key, exchange: exchange,
                                 entry_type: :adjustment, base_currency: 'BTC', base_asset_id: stock.id,
                                 base_amount: 1, quote_currency: nil, quote_amount: nil,
                                 transacted_at: Time.utc(2026, 9, 10),
                                 raw_data: { 'activity_type' => 'SPLIT', 'corporate_action' => 'split',
                                             'symbol' => 'BTC', 'split_ratio' => '2:1' })
    travel_to Time.utc(2026, 10, 6, 12) do
      before = [Ticker.asset_ids_named(exchange.id, 'BTC').sort, bot.split_events,
                bot.metrics(force: true)[:total_base_amount], AccountTransaction.order(:id).map(&:attributes)]
      stock_ticker.update_columns(ticker: 'BTC', base: 'BTC', available: true)
      bot = Bot.find(bot.id)
      after = [Ticker.asset_ids_named(exchange.id, 'BTC').sort, bot.split_events,
               bot.metrics(force: true)[:total_base_amount], AccountTransaction.order(:id).map(&:attributes)]
      assert_equal before, after
      assert_equal 1.to_d, after[2], 'the coin holding is not doubled by the security split'
    end
  end
end
