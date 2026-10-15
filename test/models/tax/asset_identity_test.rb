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

  # Asked for a coin, a broker's name never answers a security: which of the two a name is there is
  # `identity`'s question, and the price key keeps their series apart.
  test 'a coin lookup at a stock venue never answers a security' do
    coin('AAA', 'aaa-coin')
    coin('AAA', 'alpaca_aaa', category: 'Stock')
    coin('BBB', 'bbb-coin')
    coin('CCC', 'CCC.US', category: 'Stock')

    assert_equal 'aaa-coin', Tax::AssetIdentity.coin_id('AAA', exchange: @alpaca)
    assert_equal 'bbb-coin', Tax::AssetIdentity.coin_id('BBB', exchange: @alpaca)
    assert_nil Tax::AssetIdentity.coin_id('CCC', exchange: @alpaca)
  end

  test 'what a broker name is: the asset recorded, else what the venue lists, else the catalogue; both is nil' do
    usd = coin('USD', 'usd', category: 'Fiat')
    bitcoin = coin('BTC', 'bitcoin', rank: 1)
    security = coin('BTC', 'BTC.US', category: 'Stock')
    apple = coin('AAPL', 'AAPL.US', category: 'Stock')
    create(:ticker, exchange: @alpaca, base_asset: security, quote_asset: usd, ticker: 'BTC', base: 'BTC', quote: 'USD')
    create(:ticker, exchange: @alpaca, base_asset: bitcoin, quote_asset: usd, ticker: 'BTC/USD', base: 'BTC', quote: 'USD')
    create(:ticker, exchange: @alpaca, base_asset: apple, quote_asset: usd, ticker: 'AAPL', base: 'AAPL', quote: 'USD')

    assert_nil Tax::AssetIdentity.identity('BTC', exchange: @alpaca)
    assert_equal [:coin, bitcoin], Tax::AssetIdentity.identity('BTC', exchange: @alpaca, recorded: [bitcoin])
    assert_equal [:stock, security], Tax::AssetIdentity.identity('BTC', exchange: @alpaca, recorded: [security])
    assert_equal [:stock, apple], Tax::AssetIdentity.identity('AAPL', exchange: @alpaca)
    assert_equal [:coin], Tax::AssetIdentity.identity('BTC', exchange: @binance)

    assert_equal ['AAPL', apple], Tax::AssetIdentity.price_key('AAPL', exchange: @alpaca), 'no coin is AAPL: the key it always had'
    assert_equal ['stock:BTC', security], Tax::AssetIdentity.price_key('BTC', exchange: @alpaca, recorded: [security])
    assert_equal ['BTC'], Tax::AssetIdentity.price_key('BTC', exchange: @alpaca, recorded: [bitcoin])
    assert_nil Tax::AssetIdentity.price_key('BTC', exchange: @alpaca)
    assert_equal ['stock:AAPL', apple], Tax::AssetIdentity.price_key('AAPL', exchange: @alpaca, recorded_coins: true),
                 'a coin recorded under the name elsewhere owns the bare key'
  end

  test 'a venue listing only the security still keeps it off a coin\'s key' do
    usd = coin('USD', 'usd', category: 'Fiat')
    coin('BTC', 'bitcoin', rank: 1)
    security = coin('BTC', 'BTC.US', category: 'Stock')
    create(:ticker, exchange: @alpaca, base_asset: security, quote_asset: usd, ticker: 'BTC', base: 'BTC', quote: 'USD')

    assert_equal ['stock:BTC', security], Tax::AssetIdentity.price_key('BTC', exchange: @alpaca)
    assert_equal ['BTC'], Tax::AssetIdentity.price_key('BTC', exchange: @binance)
  end

  test 'a dated alias is a coin at a broker too, on both sides of its cut' do
    coin('LUNA', 'terra-luna-2', rank: 300)

    assert_equal ['LUNA'], Tax::AssetIdentity.price_key('LUNA', exchange: @alpaca, at: Time.utc(2022, 5, 27))
    assert_equal ['LUNA'], Tax::AssetIdentity.price_key('LUNA', exchange: @alpaca, at: Time.utc(2022, 5, 28))
    assert Tax::AssetIdentity.dated_alias?('LUNA', exchange: @alpaca)
    assert_not Tax::AssetIdentity.dated_alias?('MATIC', exchange: @alpaca), 'an undated alias cuts nothing'
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

  # QuickSwap redenominated QUICK 1 old = 1000 new. Binance traded the old token through 2023-07-17,
  # suspended, and reopened in new units on 2023-07-21; no venue carried the new token before that.
  # CoinGecko keeps the old token as `quick`, the new one as `quickswap`.
  test 'QUICK means the old token until venues switched' do
    coin('QUICK', 'quickswap', rank: 1500)

    assert_equal 'quick', Tax::AssetIdentity.coin_id('QUICK', exchange: @binance, at: Time.utc(2021, 9, 17))
    assert_equal 'quick', Tax::AssetIdentity.coin_id('QUICK', exchange: @binance, at: Time.utc(2023, 7, 20, 23))
    assert_equal 'quickswap', Tax::AssetIdentity.coin_id('QUICK', exchange: @binance, at: Time.utc(2023, 7, 21))
    assert_equal 'quickswap', Tax::AssetIdentity.coin_id('QUICK', exchange: create(:coinbase_exchange), at: Time.utc(2023, 9, 1)),
                 'one coin per symbol per day: prices are stored by symbol'
    assert_equal 'quickswap', Tax::AssetIdentity.coin_id('QUICK', exchange: @kraken, at: Time.utc(2024, 1, 1)),
                 'after the switch any other venue resolves through its listing or the catalogue'
    assert_equal [[Date.new(2021, 9, 4)..Date.new(2023, 7, 20), 'quick'],
                  [Date.new(2023, 7, 21)..Date.new(2024, 1, 1), 'quickswap']],
                 Tax::AssetIdentity.coin_ids_over('QUICK', exchange: @binance, from: Date.new(2021, 9, 4), to: Date.new(2024, 1, 1))
  end
end
