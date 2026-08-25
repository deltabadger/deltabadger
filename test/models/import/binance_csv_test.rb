require 'test_helper'

# Reading Binance's own transaction export.
#
# The export is what the API cannot give: Binance serves ~90 days through `myTrades` and the
# deposit/withdraw endpoints, so an account older than that has funding history nothing can fetch —
# which is what leaves the tracker inferring how much money went in.
#
# The one rule this file exists to pin: the importer must produce the SAME SHAPES as
# `Exchanges::Binance#get_ledger`. Not similar ones. A file overlaps the window the API already
# synced, and the dedup rule for a row with no id matches on
# (api_key, entry_type, base_currency, base_amount, transacted_at) — so a mapping that disagrees
# with the adapter by one field, or by one entry type, silently doubles every row in the overlap.
class Import::BinanceCsvTest < ActiveSupport::TestCase
  HEADER = 'User ID,Time,Account,Operation,Coin,Change,Remark'.freeze

  setup do
    @user = create(:user)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
  end

  def parse(*rows, offset: '+00:00')
    Import::BinanceCsv.new([HEADER, *rows].join("\n"), offset: offset).entries
  end

  # ── one trade, three rows ────────────────────────────────────────────────────────────────────
  test 'the three rows of a trade fold into the one buy the adapter would have made' do
    entries = parse(
      '1,2021-07-01 03:42:38,Spot,Transaction Fee,BNB,-0.00002482,',
      '1,2021-07-01 03:42:38,Spot,Transaction Spend,USDT,-10.0003543,',
      '1,2021-07-01 03:42:38,Spot,Transaction Buy,LTC,0.06983,'
    )

    entry = entries.sole
    assert_equal :buy, entry[:entry_type]
    assert_equal 'LTC', entry[:base_currency]
    assert_equal 0.06983.to_d, entry[:base_amount]
    assert_equal 'USDT', entry[:quote_currency]
    assert_equal 10.0003543.to_d, entry[:quote_amount]
    assert_equal 'BNB', entry[:fee_currency]
    assert_equal 0.00002482.to_d, entry[:fee_amount]
    assert_equal Time.utc(2021, 7, 1, 3, 42, 38), entry[:transacted_at]
  end

  test 'a sale folds the same way' do
    entries = parse(
      '1,2022-01-05 10:00:00,Spot,Transaction Sold,BTC,-0.5,',
      '1,2022-01-05 10:00:00,Spot,Transaction Revenue,USDT,21000,',
      '1,2022-01-05 10:00:00,Spot,Transaction Fee,USDT,-21,'
    )

    entry = entries.sole
    assert_equal :sell, entry[:entry_type]
    assert_equal 'BTC', entry[:base_currency]
    assert_equal 0.5.to_d, entry[:base_amount]
    assert_equal 21_000.to_d, entry[:quote_amount]
  end

  # A crypto-quoted trade is a SWAP to the adapter, not a buy — two rows sharing a group.
  test 'a trade with no cash leg is a swap pair, as the adapter makes it' do
    entries = parse(
      '1,2022-03-03 08:00:00,Spot,Transaction Spend,BTC,-0.01,',
      '1,2022-03-03 08:00:00,Spot,Transaction Buy,ETH,0.15,'
    )

    assert_equal(%i[swap_out swap_in], entries.map { |e| e[:entry_type] })
    assert_equal 1, entries.map { |e| e[:group_id] }.uniq.size
    assert entries.first[:group_id].present?
  end

  # ── the funding rows, which are the point ────────────────────────────────────────────────────
  test 'deposits and withdrawals come through' do
    entries = parse(
      '1,2021-07-01 14:07:04,Spot,Deposit,USDT,150.2,',
      '1,2021-08-03 17:12:44,Spot,Withdraw,LTC,-0.501,Withdraw fee is included'
    )

    assert_equal(%i[deposit withdrawal], entries.map { |e| e[:entry_type] })
    assert_equal 150.2.to_d, entries.first[:base_amount]
    assert_equal 0.501.to_d, entries.last[:base_amount], 'a withdrawal is a positive amount leaving'
  end

  # Money moving inside Binance is not money arriving. Counted as funding it would inflate the very
  # figure this import exists to correct.
  test 'moving money between one own wallet and another is not funding' do
    assert_empty parse('1,2021-09-01 12:00:00,Spot,Transfer Between Spot and Funding,USDT,-50,')
  end

  # A dust sweep is many coins into BNB at one timestamp, and Binance writes a row per leg. The
  # `Remark` — "CHZ to BNB" — is what says which BNB credit came from which coin, and it has to be
  # read: `import_dust_conversions` pairs them one for one, so collapsing a batch into a single BNB
  # receipt would leave every dust row in the overlap window unmatched and doubled.
  test 'dust pairs coin to receipt by the remark, one pair per coin' do
    entries = parse(
      '1,2022-05-01 06:00:00,Spot,Small Assets Exchange BNB,CHZ,-3,CHZ to BNB',
      '1,2022-05-01 06:00:00,Spot,Small Assets Exchange BNB,BNB,0.0004,CHZ to BNB',
      '1,2022-05-01 06:00:00,Spot,Small Assets Exchange BNB,BAT,-0.0076,BAT to BNB',
      '1,2022-05-01 06:00:00,Spot,Small Assets Exchange BNB,BNB,0.00000919,BAT to BNB'
    )

    assert_equal 4, entries.size, 'two coins swept, two pairs — not one aggregate receipt'
    chz = entries.select { |e| e[:group_id] == entries.first[:group_id] }
    assert_equal(%i[swap_out swap_in], chz.map { |e| e[:entry_type] })
    assert_equal(%w[CHZ BNB], chz.map { |e| e[:base_currency] })
    assert_equal 0.0004.to_d, chz.last[:base_amount], 'the receipt for CHZ, not the batch total'
    assert_equal 2, entries.map { |e| e[:group_id] }.uniq.size, 'each coin its own swap'
  end

  test 'rebates and rewards are income, and keep the amount they arrived with' do
    entries = parse(
      '1,2021-06-30 17:36:03,Spot,Commission Rebate,BTC,0.00000007,',
      '1,2021-11-02 09:00:00,Spot,Pool Distribution,ETH,0.001,',
      '1,2021-12-02 09:00:00,Spot,Airdrop Assets,SOL,0.5,'
    )

    assert_equal 3, entries.size
    assert_equal 0.00000007.to_d, entries.first[:base_amount]
    assert(entries.all? { |e| AccountTransaction.entry_types.key?(e[:entry_type].to_s) })
  end

  # ── several fills of one order, all sharing one second ───────────────────────────────────────
  #
  # Binance writes a Spend/Buy/Fee triple PER FILL, and the matching engine happily fills an order in
  # two pieces within the same second. Reading the group as one trade keeps the first piece and drops
  # the rest — coins that left the account silently stay in the ledger, the ledger then disagrees
  # with the balance, and every P/L on the page goes quiet because neither can be trusted.
  test 'two fills at one instant are two trades, not one' do
    entries = parse(
      '1,2022-04-24 10:28:59,Spot,Transaction Buy,HNT,22.22,',
      '1,2022-04-24 10:28:59,Spot,Transaction Spend,BTC,-0.0097768,',
      '1,2022-04-24 10:28:59,Spot,Transaction Fee,BNB,-0.00072127,',
      '1,2022-04-24 10:28:59,Spot,Transaction Spend,BTC,-0.0020988,',
      '1,2022-04-24 10:28:59,Spot,Transaction Buy,HNT,4.77,',
      '1,2022-04-24 10:28:59,Spot,Transaction Fee,BNB,-0.00015483,'
    )

    out = entries.select { |e| e[:entry_type] == :swap_out }
    assert_equal 2, out.size, 'two fills'
    assert_equal 0.0118756.to_d, out.sum { |e| e[:base_amount] }, 'and every satoshi of both'
    assert_equal(2, entries.count { |e| e[:entry_type] == :swap_in })
    assert_equal(26.99.to_d, entries.select { |e| e[:entry_type] == :swap_in }.sum { |e| e[:base_amount] })
  end

  # The rows arrive interleaved and carry no order id, so the legs are paired by size: fills of one
  # order share a price, which is what makes the largest spend belong to the largest buy. Pairing
  # them in file order instead prices a 0.00005 BTC fill at a 0.00003 BTC fill's cost.
  test 'the legs of each fill are paired by size, so the unit price comes out right' do
    entries = parse(
      '1,2025-06-29 05:04:58,Spot,Transaction Buy,BTC,0.00005,',
      '1,2025-06-29 05:04:58,Spot,Transaction Fee,BTC,-0.00000005,',
      '1,2025-06-29 05:04:58,Spot,Transaction Spend,USDC,-3.2167998,',
      '1,2025-06-29 05:04:58,Spot,Transaction Fee,BTC,-0.00000003,',
      '1,2025-06-29 05:04:58,Spot,Transaction Buy,BTC,0.00003,',
      '1,2025-06-29 05:04:58,Spot,Transaction Spend,USDC,-5.361333,'
    )

    prices = entries.map { |e| (e[:quote_amount] / e[:base_amount]).round(0) }
    assert_equal [107_227.to_d], prices.uniq, 'both fills of one order priced the same'
    assert_equal(8.5781328.to_d, entries.sum { |e| e[:quote_amount] })
  end

  # The invariant underneath all of it: a trade row in, a trade row out. Whatever the grouping does,
  # the coins it accounts for must be the coins the file states.
  test 'no trade row is ever dropped' do
    rows = [
      '1,2022-04-24 10:28:59,Spot,Transaction Buy,HNT,22.22,',
      '1,2022-04-24 10:28:59,Spot,Transaction Spend,BTC,-0.0097768,',
      '1,2022-04-24 10:28:59,Spot,Transaction Spend,BTC,-0.0020988,',
      '1,2022-04-24 10:28:59,Spot,Transaction Buy,HNT,4.77,',
      '1,2023-01-01 00:00:00,Spot,Transaction Spend,USDT,-50,',
      '1,2023-01-01 00:00:00,Spot,Transaction Buy,ETH,0.03,'
    ]
    entries = parse(*rows)

    moved = Hash.new(0.to_d)
    entries.each do |entry|
      moved[entry[:base_currency]] += entry[:base_amount]
      moved[entry[:quote_currency]] += entry[:quote_amount] if entry[:quote_currency]
    end
    assert_equal 0.0118756.to_d, moved['BTC']
    assert_equal 26.99.to_d, moved['HNT']
    assert_equal 50.to_d, moved['USDT']
    assert_equal 0.03.to_d, moved['ETH']
  end

  # ── the timezone, which is where a silent double comes from ──────────────────────────────────
  #
  # The file states a wall-clock time and NOT its zone; only the filename carries it. Read at the
  # wrong offset every row misses the rows already stored and the whole history lands twice.
  test 'the offset the file was exported at is what makes its rows line up' do
    entry = parse('1,2021-07-01 03:42:38,Spot,Deposit,USDT,150,', offset: '+03:00').sole

    assert_equal Time.utc(2021, 7, 1, 0, 42, 38), entry[:transacted_at]
  end

  test 'the offset is read off Binance\'s own filename' do
    assert_equal '+03:00',
                 Import::BinanceCsv.offset_from('Binance-Transaction-History-202608242240(UTC+3)-part1-of1.csv')
    assert_nil Import::BinanceCsv.offset_from('renamed-by-the-user.csv')
  end

  # ── the sign is the direction ────────────────────────────────────────────────────────────────
  #
  # An operation name does not say which way an asset moved; the `Change` column does. Binance takes
  # a delisted token back under the SAME name it uses to hand tokens out, so reading the name alone
  # books a loss as income — and the coins never leave the ledger, which is how a ledger comes to
  # hold twice what the venue reports.
  test 'an asset taken back leaves, however the operation is named' do
    entry = parse('1,2024-04-26 12:23:15,Spot,Asset Recovery,HNT,-31.85,').sole

    assert_equal :lost, entry[:entry_type]
    assert_equal 31.85.to_d, entry[:base_amount]
  end

  test 'the same operation handing something out is still income' do
    entry = parse('1,2024-06-26 08:26:41,Spot,Token Swap - Distribution,USDT,118.55207,').sole

    assert_equal :other_income, entry[:entry_type]
  end

  # A rename is one asset becoming another, not two windfalls: LUNA out, LUNC in, same instant.
  test 'a token rename is a swap, not two gifts' do
    entries = parse(
      '1,2022-05-28 22:57:37,Spot,Token Swap - Redenomination/Rebranding,LUNA,-12.920406,',
      '1,2022-05-28 22:57:37,Spot,Token Swap - Redenomination/Rebranding,LUNC,12.920406,'
    )

    assert_equal(%i[swap_out swap_in], entries.map { |e| e[:entry_type] })
    assert_equal(%w[LUNA LUNC], entries.map { |e| e[:base_currency] })
    assert_equal 1, entries.map { |e| e[:group_id] }.uniq.size
  end

  test 'a withdrawal is still a withdrawal, not a loss' do
    entry = parse('1,2021-08-03 17:12:44,Spot,Withdraw,LTC,-0.501,Withdraw fee is included').sole

    assert_equal :withdrawal, entry[:entry_type]
  end

  # ── legs that do not land on the same second ─────────────────────────────────────────────────
  #
  # A Convert writes its two halves a second apart. Grouped by exact timestamp they never meet, and
  # a half with nothing to pair against is silently dropped — both halves, in fact, so 129 USDC and
  # 129 USDT vanish from a history that has to reconcile to the last satoshi.
  test 'a convert whose halves are a second apart is still one trade' do
    entries = parse(
      '1,2025-06-22 20:20:45,Spot,Binance Convert,USDC,129.41567489,',
      '1,2025-06-22 20:20:46,Spot,Binance Convert,USDT,-129.41567489,'
    )

    entry = entries.sole
    assert_equal :buy, entry[:entry_type]
    assert_equal 'USDC', entry[:base_currency]
    assert_equal 129.41567489.to_d, entry[:base_amount]
    assert_equal 'USDT', entry[:quote_currency]
    assert_equal 129.41567489.to_d, entry[:quote_amount]
  end

  test 'two converts far apart are not paired with each other' do
    entries = parse(
      '1,2021-09-20 09:22:54,Spot,Binance Convert,LTC,0.89568816,',
      '1,2021-09-20 09:22:54,Spot,Binance Convert,USDT,-150,',
      '1,2024-02-17 20:07:21,Spot,Binance Convert,BNB,0.00096277,',
      '1,2024-02-17 20:07:21,Spot,Binance Convert,CHZ,-3,'
    )

    assert_equal 3, entries.size, 'a purchase, and a swap of CHZ into BNB'
    assert_equal 150.to_d, entries.find { |e| e[:base_currency] == 'LTC' }[:quote_amount]
  end

  # Binance does not always write the coins a dust sweep consumed — five BNB credits, no Remark, and
  # nothing going out. They still arrived, so they are booked as arriving; dropping them would leave
  # the ledger short of BNB the account really holds.
  test 'a credit whose source the file never states still arrives' do
    entries = parse(
      '1,2021-07-06 13:12:11,Spot,Small Assets Exchange BNB,BNB,0.00271024,',
      '1,2021-07-06 13:12:11,Spot,Small Assets Exchange BNB,BNB,0.01097549,'
    )

    assert_equal 2, entries.size
    assert_equal [:other_income], entries.map { |e| e[:entry_type] }.uniq
    assert_equal(0.01368573.to_d, entries.sum { |e| e[:base_amount] })
  end

  # ── never invent ─────────────────────────────────────────────────────────────────────────────
  test 'an operation we do not know is reported, never guessed at' do
    importer = Import::BinanceCsv.new(
      [HEADER, '1,2026-01-01 00:00:00,Spot,Some Future Binance Product,XYZ,1,'].join("\n"),
      offset: '+00:00'
    )

    assert_empty importer.entries
    assert_equal ['Some Future Binance Product'], importer.unrecognised
  end
end
