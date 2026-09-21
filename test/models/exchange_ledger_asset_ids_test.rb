require 'test_helper'

# Which asset a ledger row moved, as the venue that booked it lists it. What this returns is stored
# on the row and never overwritten, so the rule is: the venue's own listing, exactly one asset, or
# nothing — never a guess from the catalogue.
class ExchangeLedgerAssetIdsTest < ActiveSupport::TestCase
  def row(base, at: Time.utc(2026, 9, 1), raw: {})
    { base_currency: base, transacted_at: at, raw_data: raw }
  end

  def listing(exchange, base_asset, quote_asset, base: base_asset.symbol, quote: quote_asset.symbol, **attrs)
    create(:ticker, exchange: exchange, base_asset: base_asset, quote_asset: quote_asset, base: base, quote: quote,
                    ticker: "#{base}/#{quote}", **attrs)
  end

  # ── a crypto venue ─────────────────────────────────────────────────────────────────────────────
  class CryptoVenueTest < ExchangeLedgerAssetIdsTest
    setup do
      @binance = create(:binance_exchange)
      @usdt = create(:asset, :usdt)
    end

    test 'a name the venue lists as one asset resolves to it, an unavailable listing included' do
      ronin = create(:asset, external_id: 'ronin', symbol: 'RON')
      listing(@binance, ronin, @usdt, base: 'RONIN', available: false)

      assert_equal [ronin.id], @binance.ledger_asset_ids([row('RONIN')])
    end

    test 'a name the venue lists as two assets stays nil' do
      one = create(:asset, external_id: 'portuma', symbol: 'POR')
      other = create(:asset, external_id: 'portugal-fan-token', symbol: 'POR')
      listing(@binance, one, @usdt)
      listing(@binance, other, create(:asset, :bitcoin))

      assert_equal [nil], @binance.ledger_asset_ids([row('POR')])
    end

    test 'a name no listing carries stays nil even when the catalogue has a coin by that symbol' do
      create(:asset, external_id: 'helium', symbol: 'HNT')

      assert_equal [nil], @binance.ledger_asset_ids([row('HNT')])
    end

    # LUNA before the collapse is the coin now called LUNC; the venue lists today's LUNA under the
    # same name. The row's own date decides, which is what makes an old file safe to import.
    test 'a dated alias beats today\'s listing' do
      classic = create(:asset, external_id: 'terra-luna', symbol: 'LUNC')
      today = create(:asset, external_id: 'terra-luna-2', symbol: 'LUNA')
      listing(@binance, today, @usdt)

      assert_equal [classic.id, today.id],
                   @binance.ledger_asset_ids([row('LUNA', at: Time.utc(2021, 6, 1)), row('LUNA', at: Time.utc(2023, 6, 1))])
    end

    test 'an alias whose coin is not in the catalogue stays nil rather than taking the listing' do
      pol = create(:asset, external_id: 'polygon-ecosystem-token', symbol: 'POL')
      listing(@binance, pol, @usdt, base: 'MATIC')

      assert_equal [nil], @binance.ledger_asset_ids([row('MATIC')])
    end
  end

  # ── a broker that trades coins too ─────────────────────────────────────────────────────────────
  class AlpacaTest < ExchangeLedgerAssetIdsTest
    setup do
      @alpaca = create(:alpaca_exchange)
      @usd = create(:asset, :usd)
      @bitcoin = create(:asset, :bitcoin)
      @ethereum = create(:asset, :ethereum)
    end

    def stock(symbol, external_id: "#{symbol}.US", listed: true)
      create(:asset, external_id: external_id, symbol: symbol, category: 'Stock').tap do |asset|
        listing(@alpaca, asset, @usd) if listed
      end
    end

    def fill(symbol, base = symbol) = row(base, raw: { 'activity_type' => 'FILL', 'symbol' => symbol })

    test 'a sold stock\'s fill is its own listing, never a same-symbol coin or another listing' do
      stock('DASH', external_id: 'DASH.TO', listed: false)
      create(:asset, external_id: 'dash', symbol: 'DASH', category: 'Cryptocurrency')
      doordash = stock('DASH')

      assert_equal [doordash.id], @alpaca.ledger_asset_ids([fill('DASH')])
    end

    # The (base, quote) slot is unique per venue, so where Alpaca lists the bitcoin ETF as BTC/USD
    # the coin has no ticker of its own. The activity's symbol still says which one was traded.
    test 'a BTC/USD fill is the coin where the only BTC ticker is the ETF, and a bare BTC fill is the ETF' do
      etf = stock('BTC')

      assert_equal [@bitcoin.id, etf.id], @alpaca.ledger_asset_ids([fill('BTC/USD', 'BTC'), fill('BTC')])
    end

    # With no crypto listing available when the ledger was read, the adapter could not split the
    # compact pair and stored it whole. The raw symbol still names the coin.
    test 'a compact crypto fill and a CFEE are the coin, even where no crypto listing exists' do
      stock('ETH')

      rows = [fill('ETHUSD', 'ETH'), fill('BTCUSD'),
              row('ETH', raw: { 'activity_type' => 'CFEE', 'symbol' => 'ETHUSD' }),
              row('ETHUSD', raw: { 'activity_type' => 'CFEE', 'symbol' => 'ETHUSD' })]
      assert_equal [@ethereum.id, @bitcoin.id, @ethereum.id, @ethereum.id], @alpaca.ledger_asset_ids(rows)
    end

    # A coin Alpaca listed after the curated map was last updated: only its listing knows it.
    test 'a crypto fill takes the venue\'s own coin listing, a replaced one included' do
      mana = create(:asset, external_id: 'decentraland', symbol: 'MANA')
      listing(@alpaca, mana, @usd, base: '__stale_3_MANA', available: false)

      assert_equal [mana.id, mana.id], @alpaca.ledger_asset_ids([fill('MANA/USD', 'MANA'), fill('MANAUSD')])
    end

    # The curated map speaks only where the venue lists no coin by that name. Two listings naming
    # two coins is conflicting evidence, and a conflict is not settled by a map.
    test 'a coin name two listings disagree about stays nil' do
      solana = create(:asset, external_id: 'solana', symbol: 'SOL')
      listing(@alpaca, create(:asset, external_id: 'old-sol', symbol: 'SOL'), @usd, base: '__stale_4_SOL', available: false)
      listing(@alpaca, solana, @usd)

      assert_equal [nil], @alpaca.ledger_asset_ids([fill('SOL/USD', 'SOL')])
    end

    test 'a USD fill is the security that trades as USD; cash, fees and dividends record nothing' do
      proshares = stock('USD')
      rows = [
        fill('USD'),
        row('USD', raw: { 'activity_type' => 'CSD' }),
        row('USD', raw: { 'activity_type' => 'FEE' }),
        row('USD', raw: { 'activity_type' => 'DIV', 'symbol' => 'USD' }).merge(quote_currency: 'USD'),
        row('USD', raw: { 'activity_type' => 'DIV', 'symbol' => 'AAPL' })
      ]

      assert_equal [proshares.id, nil, nil, nil, nil], @alpaca.ledger_asset_ids(rows)
    end

    test 'a split names its security' do
      snps = stock('SNPS')

      assert_equal [snps.id], @alpaca.ledger_asset_ids([row('SNPS', raw: { 'activity_type' => 'SPLIT', 'symbol' => 'SNPS' })])
    end

    # A row imported from a file carries no activity: a share's name still resolves, a name Alpaca
    # also trades as a coin could be either and stays nil.
    test 'a row with no activity resolves a share, and nothing for a coin\'s name or cash' do
      snps = stock('SNPS')
      stock('BTC')

      assert_equal [snps.id, nil, nil], @alpaca.ledger_asset_ids([row('SNPS'), row('BTC'), row('USD')])
    end
  end
end
