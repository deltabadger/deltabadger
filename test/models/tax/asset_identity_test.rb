require 'test_helper'

# Which coin a symbol means. `assets.symbol` is not unique, a venue's ticker is not a coin id, and
# a symbol can change coin over time — and the price a row gets is only as right as that answer.
class Tax::AssetIdentityTest < ActiveSupport::TestCase
  setup do
    @binance = create(:binance_exchange)
    @kraken = create(:kraken_exchange)
    @alpaca = create(:alpaca_exchange)
  end

  def coin(symbol, external_id, category: 'Cryptocurrency', rank: nil)
    create(:asset, symbol: symbol, name: external_id, external_id: external_id, category: category, market_cap_rank: rank)
  end

  test 'a crypto venue never means a stock' do
    coin('XYZ', 'alpaca_xyz', category: 'Stock')

    assert_nil Tax::AssetIdentity.coin_id('XYZ', exchange: @binance)

    coin('XYZ', 'xyz-coin')
    assert_equal 'xyz-coin', Tax::AssetIdentity.coin_id('XYZ', exchange: @binance)
  end

  test 'a stock venue means the stock first, and the coin where there is no stock' do
    coin('AAA', 'aaa-coin')
    coin('AAA', 'alpaca_aaa', category: 'Stock')
    coin('BBB', 'bbb-coin')

    assert_equal 'alpaca_aaa', Tax::AssetIdentity.coin_id('AAA', exchange: @alpaca)
    assert_equal 'bbb-coin', Tax::AssetIdentity.coin_id('BBB', exchange: @alpaca)
  end

  test 'what the venue lists under a symbol is what it means there; elsewhere, the coin by rank' do
    big = coin('XYZ', 'xyz-big', rank: 5)
    small = coin('XYZ', 'xyz-small', rank: 900)
    create(:ticker, exchange: @binance, base_asset: small, quote_asset: create(:asset, symbol: 'USDT', name: 'Tether'))

    assert_equal 'xyz-small', Tax::AssetIdentity.coin_id('XYZ', exchange: @binance), 'Binance lists the small one'
    assert_equal 'xyz-big', Tax::AssetIdentity.coin_id('XYZ', exchange: @kraken)
    assert_equal 'xyz-big', Tax::AssetIdentity.coin_id('XYZ')
    assert_equal big.external_id, Tax::AssetIdentity.coin_id('XYZ')
  end

  test 'a symbol that changed coin means the coin it was that day' do
    coin('LUNA', 'terra-luna-2')

    assert_equal 'terra-luna', Tax::AssetIdentity.coin_id('LUNA', exchange: @binance, at: Time.utc(2021, 9, 10))
    assert_equal 'terra-luna-2', Tax::AssetIdentity.coin_id('LUNA', exchange: @binance, at: Time.utc(2025, 6, 22))
    assert_equal 'terra-luna-2', Tax::AssetIdentity.coin_id('LUNA', exchange: @binance), 'no day asked: the coin it is'
    assert_equal [[Date.new(2021, 9, 1)..Date.new(2022, 5, 27), 'terra-luna'],
                  [Date.new(2022, 5, 28)..Date.new(2025, 6, 30), 'terra-luna-2']],
                 Tax::AssetIdentity.coin_ids_over('LUNA', exchange: @binance, from: Date.new(2021, 9, 1), to: Date.new(2025, 6, 30))
  end

  test 'a symbol the catalogue dropped means its successor' do
    assert_equal 'matic-network', Tax::AssetIdentity.coin_id('MATIC', exchange: @binance, at: Time.utc(2021, 7, 1))
  end

  test 'an alias scoped to a venue speaks only there' do
    coin('LIT', 'lighter', rank: 77)

    assert_equal 'litentry', Tax::AssetIdentity.coin_id('LIT', exchange: @binance)
    assert_equal 'lighter', Tax::AssetIdentity.coin_id('LIT', exchange: @kraken)
  end
end
