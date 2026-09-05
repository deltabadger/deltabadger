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
  # B10. The self-hosted twin of data-api's join bug: the CoinGecko symbol->coin_id map is keyed on
  # CoinGecko's casing while the venue reports its own. Bitget publishes baseCoin "rON" where
  # CoinGecko says "RON", so external_id_from_symbol missed and `next if blank?` dropped a live
  # pair with no trace. Fixing only data-api leaves every CoinGecko-provider install blind.
  test 'resolves a CoinGecko symbol to a venue symbol case-insensitively' do
    ron = create(:asset, symbol: 'RON', name: 'Ronin', external_id: 'ronin')
    @exchange.send(:set_symbol_to_external_id_hash,
                   [{ 'base' => 'RON', 'coin_id' => 'ronin', 'target' => 'USD', 'target_coin_id' => 'usd' }])

    sync([{ base: 'rON', quote: 'USD', ticker: 'rONUSD', available: true, trading_enabled: true,
            minimum_base_size: 0.1, minimum_quote_size: 5, maximum_base_size: nil, maximum_quote_size: nil,
            base_decimals: 4, quote_decimals: 2, price_decimals: 2 }])

    ticker = @exchange.tickers.find_by(base: 'rON', quote: 'USD')
    assert_not_nil ticker, 'a mixed-case venue symbol must still resolve'
    assert_equal ron.id, ticker.base_asset_id
  end
end
