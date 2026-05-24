require 'test_helper'

# Covers the available (listed) vs trading_enabled (native status) contract in the
# self-hosted/CoinGecko sync path. Returned pairs are always re-listed; native status
# rides on trading_enabled; pairs absent from the feed are swept to available: false.
class Exchange::SynchronizerTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @eth = create(:asset, :ethereum)
  end

  def sync(tickers_info)
    @exchange.send(:sync_existing_exchange_assets_and_tickers!, tickers_info)
  end

  test 'a returned halted pair stays listed but is marked not trading_enabled' do
    ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                             base: 'BTC', quote: 'USD', ticker: 'BTCUSD')

    sync([{ base: 'BTC', quote: 'USD', available: true, trading_enabled: false }])

    ticker.reload
    assert ticker.available, 'returned pair stays listed'
    assert_not ticker.trading_enabled, 'native halted status rides on trading_enabled'
  end

  test 'a previously delisted pair is re-listed when it returns in the feed' do
    ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                             base: 'BTC', quote: 'USD', ticker: 'BTCUSD', available: false)

    sync([{ base: 'BTC', quote: 'USD', available: true, trading_enabled: true }])

    assert ticker.reload.available, 'returned pair is re-listed'
    assert ticker.trading_enabled
  end

  test 'a pair absent from the feed is swept to available: false' do
    btc_usd = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd,
                              base: 'BTC', quote: 'USD', ticker: 'BTCUSD')
    eth_usd = create(:ticker, exchange: @exchange, base_asset: @eth, quote_asset: @usd,
                              base: 'ETH', quote: 'USD', ticker: 'ETHUSD')

    # Only BTC/USD is returned this sync.
    sync([{ base: 'BTC', quote: 'USD', available: true, trading_enabled: true }])

    assert btc_usd.reload.available
    assert_not eth_usd.reload.available, 'pair absent from the feed is delisted'
  end
end
