require 'test_helper'

class ExchangeAssetFromSymbolTest < ActiveSupport::TestCase
  # Characterization tests for the asset_from_symbol lookup duplicated across
  # the exchange subclasses (dedup slice 04, item 1), pinning current behavior
  # before extraction into the Exchange base class. Kraken intentionally
  # normalizes its symbols first — that override must survive the extraction.

  test 'resolves base and quote symbols to their assets on Binance' do
    exchange = create(:binance_exchange)
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    create(:ticker, exchange: exchange, base_asset: btc, quote_asset: usd)

    assert_equal btc, exchange.send(:asset_from_symbol, 'BTC')
    assert_equal usd, exchange.send(:asset_from_symbol, 'USD')
  end

  test 'returns nil for an unknown symbol' do
    exchange = create(:binance_exchange)
    create(:ticker, exchange: exchange)

    assert_nil exchange.send(:asset_from_symbol, 'NOPE')
  end

  test 'does not resolve symbols that only appear on unavailable tickers' do
    exchange = create(:coinbase_exchange)
    delisted = create(:asset)
    create(:ticker, exchange: exchange, base_asset: delisted, available: false)

    assert_nil exchange.send(:asset_from_symbol, delisted.symbol)
  end

  test "Kraken strips staking suffixes: 'ETH.S' resolves to the ETH asset" do
    exchange = create(:kraken_exchange)
    eth = create(:asset, :ethereum)
    create(:ticker, exchange: exchange, base_asset: eth)

    assert_equal eth, exchange.send(:asset_from_symbol, 'ETH.S')
  end

  test "Kraken maps balance codes through ASSET_MAP: 'XXBT' resolves to the BTC asset via XBT" do
    exchange = create(:kraken_exchange)
    btc = create(:asset, :bitcoin)
    create(:ticker, exchange: exchange, base_asset: btc, base_symbol: 'XBT')

    assert_equal btc, exchange.send(:asset_from_symbol, 'XXBT')
  end
end
