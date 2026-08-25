require 'test_helper'

class Tax::Methods::FifoCryptoToCryptoTest < ActiveSupport::TestCase
  setup do
    @fifo = Tax::Methods::Fifo.new
  end

  test 'crypto-to-crypto not taxable: swap_out does not create disposal' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :swap_out, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-out',
        exchange: 'binance', group_id: 'swap_1', quote_currency: nil },
      { entry_type: :swap_in, base_currency: 'ETH', base_amount: 20.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-in',
        exchange: 'binance', group_id: 'swap_1' }
    ]

    disposals = @fifo.calculate(transactions, crypto_to_crypto_taxable: false)
    assert_empty disposals
  end

  test 'cost basis chains through swap' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :swap_out, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-out',
        exchange: 'binance', group_id: 'swap_1', quote_currency: nil },
      { entry_type: :swap_in, base_currency: 'ETH', base_amount: 20.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-in',
        exchange: 'binance', group_id: 'swap_1' },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 20.to_d,
        fiat_value: 60_000.to_d, transacted_at: Time.utc(2024, 9, 1), tx_id: 'sell-1',
        exchange: 'binance', quote_currency: 'EUR' }
    ]

    disposals = @fifo.calculate(transactions, crypto_to_crypto_taxable: false)

    assert_equal 1, disposals.size
    # Cost basis should be original BTC purchase (10000), not ETH FMV at swap (50000)
    assert_equal 10_000.to_d, disposals.first[:cost_basis]
    assert_equal 50_000.to_d, disposals.first[:gain_loss]
  end

  test 'multi-hop swap chains cost basis' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      # BTC → ETH
      { entry_type: :swap_out, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'swap1-out',
        exchange: 'binance', group_id: 'swap_1', quote_currency: nil },
      { entry_type: :swap_in, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 3, 1), tx_id: 'swap1-in',
        exchange: 'binance', group_id: 'swap_1' },
      # ETH → SOL
      { entry_type: :swap_out, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap2-out',
        exchange: 'binance', group_id: 'swap_2', quote_currency: nil },
      { entry_type: :swap_in, base_currency: 'SOL', base_amount: 200.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap2-in',
        exchange: 'binance', group_id: 'swap_2' },
      # SOL → EUR
      { entry_type: :sell, base_currency: 'SOL', base_amount: 200.to_d,
        fiat_value: 40_000.to_d, transacted_at: Time.utc(2024, 9, 1), tx_id: 'sell-1',
        exchange: 'binance', quote_currency: 'EUR' }
    ]

    disposals = @fifo.calculate(transactions, crypto_to_crypto_taxable: false)

    assert_equal 1, disposals.size
    # Original cost: 10000 BTC purchase. Chained through BTC→ETH→SOL→EUR
    assert_equal 10_000.to_d, disposals.first[:cost_basis]
    assert_equal 30_000.to_d, disposals.first[:gain_loss]
  end

  test 'swap resets holding period when flag set' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :swap_out, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 8, 1), tx_id: 'swap-out',
        exchange: 'binance', group_id: 'swap_1', quote_currency: nil },
      { entry_type: :swap_in, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 20_000.to_d, transacted_at: Time.utc(2024, 8, 1), tx_id: 'swap-in',
        exchange: 'binance', group_id: 'swap_1' },
      { entry_type: :sell, base_currency: 'ETH', base_amount: 10.to_d,
        fiat_value: 30_000.to_d, transacted_at: Time.utc(2025, 3, 1), tx_id: 'sell-1',
        exchange: 'binance', quote_currency: 'EUR' }
    ]

    disposals = @fifo.calculate(transactions,
                                crypto_to_crypto_taxable: false, swap_resets_holding_period: true)

    assert_equal 1, disposals.size
    # Holding period from swap date (Aug 1) not buy date (Jan 1)
    # Aug 1 to Mar 1 = 212 days
    assert_equal 212, disposals.first[:holding_days]
    # Cost basis still from original buy
    assert_equal 10_000.to_d, disposals.first[:cost_basis]
  end

  test 'stablecoin disposal treated as fiat when flag set' do
    transactions = [
      { entry_type: :buy, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 10_000.to_d, transacted_at: Time.utc(2024, 1, 1), tx_id: 'buy-1', exchange: 'binance' },
      { entry_type: :swap_out, base_currency: 'BTC', base_amount: 1.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-out',
        exchange: 'binance', quote_currency: 'USDT', group_id: 'swap_1' },
      { entry_type: :swap_in, base_currency: 'USDT', base_amount: 50_000.to_d,
        fiat_value: 50_000.to_d, transacted_at: Time.utc(2024, 6, 1), tx_id: 'swap-in',
        exchange: 'binance', group_id: 'swap_1' }
    ]

    # Without stablecoin_as_fiat: USDT is crypto, swap not taxable
    disposals = @fifo.calculate(transactions, crypto_to_crypto_taxable: false)
    assert_empty disposals

    # With stablecoin_as_fiat: USDT treated as fiat exit
    disposals = @fifo.calculate(transactions, crypto_to_crypto_taxable: false, stablecoin_as_fiat: true)
    assert_equal 1, disposals.size
    assert_equal 40_000.to_d, disposals.first[:gain_loss]
  end

  # ── groups with more than one leg a side ──────────────────────────────────────────────────
  #
  # A Binance dust sweep is many coins into one BNB credit; a swap filled in pieces is one coin
  # into several credits. Basis has to be CONSERVED through either: whatever the out-legs gave
  # up is exactly what the in-legs receive, shared out, never the last leg's cost handed to every
  # in-leg. Groups are a venue's own, so the same id on another exchange is another group.
  def row(type, asset, amount, value, at:, group: nil, exchange: 'binance', **extra)
    { entry_type: type, base_currency: asset, base_amount: amount.to_d, fiat_value: value.to_d,
      transacted_at: at, tx_id: "#{type}-#{asset}-#{amount}-#{at.to_i}", exchange: exchange,
      group_id: group, quote_currency: nil }.merge(extra)
  end

  T1 = Time.utc(2024, 1, 1)
  T2 = Time.utc(2024, 6, 1)
  T3 = Time.utc(2024, 9, 1)

  test 'a sweep of several coins into one credit carries every leg\'s basis' do
    disposals = @fifo.calculate([
                                  row(:buy, 'ADA', 10, 100, at: T1),
                                  row(:buy, 'DOT', 5, 50, at: T1),
                                  row(:swap_out, 'ADA', 10, 120, at: T2, group: 'dust'),
                                  row(:swap_out, 'DOT', 5, 60, at: T2, group: 'dust'),
                                  row(:swap_in, 'BNB', 1, 180, at: T2, group: 'dust'),
                                  row(:sell, 'BNB', 1, 300, at: T3, quote_currency: 'EUR')
                                ], crypto_to_crypto_taxable: false)

    assert_equal 150.to_d, disposals.sole[:cost_basis], 'both legs, not whichever was stored last'
  end

  test 'a swap credited in several fills shares the basis by amount' do
    disposals = @fifo.calculate([
                                  row(:buy, 'BTC', 1, 10_000, at: T1),
                                  row(:swap_out, 'BTC', 1, 10_000, at: T2, group: 'fill'),
                                  row(:swap_in, 'HNT', 4, 4_000, at: T2, group: 'fill'),
                                  row(:swap_in, 'HNT', 6, 6_000, at: T2, group: 'fill'),
                                  row(:sell, 'HNT', 6, 9_000, at: T3, quote_currency: 'EUR'),
                                  row(:sell, 'HNT', 4, 5_000, at: T3 + 1, quote_currency: 'EUR')
                                ], crypto_to_crypto_taxable: false)

    assert_equal [6_000.to_d, 4_000.to_d], disposals.map { |d| d[:cost_basis] },
                 '10,000 went in and 10,000 comes out — not 10,000 per fill'
  end

  test 'in-legs of different assets share the basis by value' do
    disposals = @fifo.calculate([
                                  row(:buy, 'BTC', 1, 10_000, at: T1),
                                  row(:swap_out, 'BTC', 1, 10_000, at: T2, group: 'split'),
                                  row(:swap_in, 'ETH', 10, 7_500, at: T2, group: 'split'),
                                  row(:swap_in, 'SOL', 100, 2_500, at: T2, group: 'split'),
                                  row(:sell, 'ETH', 10, 8_000, at: T3, quote_currency: 'EUR'),
                                  row(:sell, 'SOL', 100, 3_000, at: T3, quote_currency: 'EUR')
                                ], crypto_to_crypto_taxable: false)

    assert_equal([7_500.to_d, 2_500.to_d], disposals.map { |d| d[:cost_basis] })
  end

  test 'a share nobody can weigh is split evenly and marked assumed' do
    disposals = @fifo.calculate([
                                  row(:buy, 'BTC', 1, 10_000, at: T1),
                                  row(:swap_out, 'BTC', 1, 10_000, at: T2, group: 'split'),
                                  row(:swap_in, 'ETH', 10, 7_500, at: T2, group: 'split'),
                                  row(:swap_in, 'SOL', 100, 0, at: T2, group: 'split', price_missing: true),
                                  row(:sell, 'ETH', 10, 8_000, at: T3, quote_currency: 'EUR'),
                                  row(:sell, 'SOL', 100, 3_000, at: T3, quote_currency: 'EUR')
                                ], crypto_to_crypto_taxable: false)

    assert_equal([5_000.to_d, 5_000.to_d], disposals.map { |d| d[:cost_basis] })
    assert disposals.all? { |d| d[:data_incomplete] }, 'an even split is a guess, and both halves say so'
  end

  test 'the same group id on another exchange is another group' do
    disposals = @fifo.calculate([
                                  row(:buy, 'BTC', 1, 10_000, at: T1),
                                  row(:swap_out, 'BTC', 1, 10_000, at: T2, group: 'r1'),
                                  row(:swap_in, 'ETH', 10, 20_000, at: T2, group: 'r1', exchange: 'kraken'),
                                  row(:sell, 'ETH', 10, 25_000, at: T3, quote_currency: 'EUR', exchange: 'kraken')
                                ], crypto_to_crypto_taxable: false)

    assert_equal 20_000.to_d, disposals.sole[:cost_basis], 'nothing chained across venues'
  end

  test 'an out-leg the lots do not cover chains an assumed basis' do
    disposals = @fifo.calculate([
                                  row(:swap_out, 'BTC', 1, 10_000, at: T2, group: 'g'),
                                  row(:swap_in, 'ETH', 10, 20_000, at: T2, group: 'g'),
                                  row(:sell, 'ETH', 10, 25_000, at: T3, quote_currency: 'EUR')
                                ], crypto_to_crypto_taxable: false)

    assert_equal 0.to_d, disposals.sole[:cost_basis]
    assert disposals.sole[:data_incomplete], 'a zero-basis lot from coins never held is not a complete one'
  end

  test 'a fee on the swap-in nets what arrived under either rule' do
    rows = [
      row(:buy, 'BTC', 1, 10_000, at: T1),
      row(:swap_out, 'BTC', 1, 10_000, at: T2, group: 'g'),
      row(:swap_in, 'ETH', 10, 20_000, at: T2, group: 'g', fee_currency: 'ETH', fee_amount: 0.1.to_d)
    ]

    [true, false].each do |taxable|
      fifo = Tax::Methods::Fifo.new
      fifo.calculate(rows, crypto_to_crypto_taxable: taxable)
      assert_equal 9.9.to_d, fifo.lots['ETH'].sum { |lot| lot[:amount] }, "taxable=#{taxable}"
    end
  end

  test 'a fee paid in a third asset on the swap-in leaves that asset and joins the cost' do
    @fifo.calculate([
                      row(:buy, 'BNB', 1, 100, at: T1),
                      row(:buy, 'BTC', 1, 10_000, at: T1),
                      row(:swap_out, 'BTC', 1, 10_000, at: T2, group: 'g'),
                      row(:swap_in, 'ETH', 10, 20_000, at: T2, group: 'g', fee_currency: 'BNB', fee_amount: 0.1.to_d,
                                                       fee_fiat_value: 10.to_d)
                    ], crypto_to_crypto_taxable: false)

    assert_equal(0.9.to_d, @fifo.lots['BNB'].sum { |lot| lot[:amount] })
    assert_equal(10_010.to_d, @fifo.lots['ETH'].sum { |lot| lot[:amount] * lot[:cost_per_unit] })
  end

  # ── cash legs ────────────────────────────────────────────────────────────────────────────
  #
  # `enrich` values a coin leg from the cash leg opposite it and leaves the cash share on the row
  # (`swap_fiat_cost`, `swap_stable_cost`). The stablecoin share is cash only under the flag.
  test 'under the flag a stablecoin swap leg is cash: cash spent is the basis and no lot is touched' do
    disposals = @fifo.calculate([
                                  row(:deposit, 'USDT', 500, 500, at: T1),
                                  row(:swap_out, 'USDT', 100, 100, at: T2, group: 'c'),
                                  row(:swap_in, 'LTC', 1, 100, at: T2, group: 'c', swap_stable_cost: 100.to_d),
                                  row(:sell, 'LTC', 1, 150, at: T3, quote_currency: 'EUR')
                                ], crypto_to_crypto_taxable: false, stablecoin_as_fiat: true)

    assert_equal 100.to_d, disposals.sole[:cost_basis]
    assert_equal 500.to_d, @fifo.lots['USDT'].sum { |lot| lot[:amount] }, 'cash is not inventory'
  end

  test 'without the flag a stablecoin leg is a coin leg and chains as one, never both' do
    disposals = @fifo.calculate([
                                  row(:deposit, 'USDT', 500, 500, at: T1),
                                  row(:swap_out, 'USDT', 100, 100, at: T2, group: 'c'),
                                  row(:swap_in, 'LTC', 1, 100, at: T2, group: 'c', swap_stable_cost: 100.to_d),
                                  row(:sell, 'LTC', 1, 150, at: T3, quote_currency: 'EUR')
                                ], crypto_to_crypto_taxable: false)

    assert_equal 100.to_d, disposals.sole[:cost_basis], 'the lot\'s basis, not the lot\'s basis plus the face'
    assert_equal(400.to_d, @fifo.lots['USDT'].sum { |lot| lot[:amount] })
  end

  test 'a mixed sweep adds the cash to the chained basis, and the taxable rule keeps the market' do
    rows = [
      row(:buy, 'ETH', 1, 300, at: T1),
      row(:swap_out, 'ETH', 1, 400, at: T2, group: 'mix'),
      row(:swap_in, 'BNB', 2, 500, at: T2, group: 'mix', swap_fiat_cost: 100.to_d),
      row(:sell, 'BNB', 2, 600, at: T3, quote_currency: 'EUR')
    ]

    chained = Tax::Methods::Fifo.new.calculate(rows, crypto_to_crypto_taxable: false)
    taxable = Tax::Methods::Fifo.new.calculate(rows, crypto_to_crypto_taxable: true)

    assert_equal 400.to_d, chained.sole[:cost_basis], '300 of ETH basis and 100 of cash'
    assert_equal 500.to_d, taxable.last[:cost_basis], 'market value, the same as any acquisition'
  end

  # ── holding periods travel with the basis ─────────────────────────────────────────────────
  #
  # Austria reads the acquisition date off every disposal, and old stock is decided by it. Coins
  # swept from lots of different ages have to arrive as lots of those ages — not as one lot dated
  # by the oldest, which would make the whole of a later sale look like old stock.
  OLD = Time.utc(2020, 6, 1)

  test 'a swap keeps each source lot\'s date' do
    disposals = @fifo.calculate([
                                  row(:buy, 'BTC', 1, 10_000, at: OLD),
                                  row(:buy, 'BTC', 1, 30_000, at: T1),
                                  row(:swap_out, 'BTC', 2, 80_000, at: T2, group: 'g'),
                                  row(:swap_in, 'ETH', 20, 80_000, at: T2, group: 'g'),
                                  row(:sell, 'ETH', 10, 25_000, at: T3, quote_currency: 'EUR'),
                                  row(:sell, 'ETH', 10, 25_000, at: T3 + 1, quote_currency: 'EUR')
                                ], crypto_to_crypto_taxable: false)

    assert_equal [[OLD, 10_000.to_d], [T1, 30_000.to_d]],
                 disposals.map { |d| [d[:acquisition_date], d[:cost_basis]] },
                 'the older half is sold first, and is the older half'
  end

  test 'a swap credited in several fills gives every fill its share of every source lot' do
    @fifo.calculate([
                      row(:buy, 'BTC', 1, 10_000, at: OLD),
                      row(:buy, 'BTC', 1, 30_000, at: T1),
                      row(:swap_out, 'BTC', 2, 80_000, at: T2, group: 'fill'),
                      row(:swap_in, 'HNT', 4, 32_000, at: T2, group: 'fill'),
                      row(:swap_in, 'HNT', 6, 48_000, at: T2, group: 'fill')
                    ], crypto_to_crypto_taxable: false)

    by_date = @fifo.lots['HNT'].group_by { |lot| lot[:date] }
                               .transform_values do |lots|
      [lots.sum { |l| l[:amount] }, lots.sum do |l|
        l[:amount] * l[:cost_per_unit]
      end]
    end

    assert_equal({ OLD => [5.to_d, 10_000.to_d], T1 => [5.to_d, 30_000.to_d] }, by_date)
  end

  test 'a tranche older than what is already held is sold first' do
    disposals = @fifo.calculate([
                                  row(:buy, 'BNB', 1, 400, at: T1),
                                  row(:buy, 'ADA', 10, 100, at: OLD),
                                  row(:swap_out, 'ADA', 10, 120, at: T2, group: 'dust'),
                                  row(:swap_in, 'BNB', 1, 120, at: T2, group: 'dust'),
                                  row(:sell, 'BNB', 1, 500, at: T3, quote_currency: 'EUR')
                                ], crypto_to_crypto_taxable: false)

    assert_equal [OLD, 100.to_d], disposals.sole.values_at(:acquisition_date, :cost_basis),
                 'the ADA-dated lot is the older one, whatever order the lots were opened in'
  end

  # ── units nobody could price ──────────────────────────────────────────────────────────────
  #
  # A lot that opened at a price nobody had is taken at zero cost, and the page says so — which
  # needs the UNITS to travel: through the tranche a swap hands over, into the lot it opens, and
  # out again with the disposal that consumes them, with that disposal's proceeds attributed to
  # them in proportion. A whole disposal's proceeds for a partly unpriced lot would overstate it.
  test 'unpriced units travel through a swap and into the disposal that consumes them' do
    disposals = @fifo.calculate([
                                  row(:airdrop, 'ZZZ', 10, 0, at: T1, price_missing: true),
                                  row(:buy, 'ZZZ', 10, 100, at: T1 + 1),
                                  row(:swap_out, 'ZZZ', 20, 250, at: T2, group: 'g'),
                                  row(:swap_in, 'BNB', 2, 250, at: T2, group: 'g'),
                                  row(:sell, 'BNB', 1.5, 450, at: T3, quote_currency: 'EUR')
                                ], crypto_to_crypto_taxable: false)

    sale = disposals.sole
    assert_equal 1.to_d, sale[:unpriced_quantity], 'the first BNB came wholly from the airdrop, the next half from the purchase'
    assert_equal 300.to_d, sale[:unpriced_proceeds], 'two thirds of the sale, in proportion'
    assert_equal(0.5.to_d, @fifo.lots['BNB'].sum { |lot| lot[:amount] })
    assert_equal 0.to_d, @fifo.lots['BNB'].sum { |lot| lot[:unpriced] }, 'what is left is the priced half'
  end
end
