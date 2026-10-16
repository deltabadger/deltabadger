require 'test_helper'

# Importing a file, twice, and over what the API already synced.
#
# The whole point: a file may be imported repeatedly and the system recognises what it already
# holds. There are no ids in a Binance export to lean on, so recognition is the identity the sync
# has always used for an id-less row — (api_key, entry_type, base_currency, base_amount,
# transacted_at) — reached through the SAME writer the sync uses, so the two cannot drift apart.
class Import::RunTest < ActiveSupport::TestCase
  HEADER = 'User ID,Time,Account,Operation,Coin,Change,Remark'.freeze
  FILE = [
    HEADER,
    '1,2021-07-01 14:07:04,Spot,Deposit,USDT,150.2,',
    '1,2021-07-01 03:42:38,Spot,Transaction Fee,BNB,-0.00002482,',
    '1,2021-07-01 03:42:38,Spot,Transaction Spend,USDT,-10.0003543,',
    '1,2021-07-01 03:42:38,Spot,Transaction Buy,LTC,0.06983,'
  ].join("\n").freeze

  setup do
    @user = create(:user)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
  end

  # NOT `run`: Minitest::Runnable#run is the method that runs the test.
  def import(text = FILE, format: 'binance', offset: '+00:00')
    Import::Run.new(user: @user, api_key: @key, format: format, text: text, offset: offset).import!
  end

  test 'a file lands' do
    result = import

    assert_equal 2, result[:imported]
    assert_equal 2, AccountTransaction.for_user(@user).count
    assert_equal %w[buy deposit], AccountTransaction.for_user(@user).pluck(:entry_type).sort
  end

  # The export and the API disagree on a transfer: a deposit is dated when it was credited, a
  # withdrawal is gross at its completion. Over a synced history, the file's copies are recognised.
  test 'a file over synced transfers adds none of them' do
    seen = Time.utc(2021, 7, 1, 14, 6, 41)
    deposit = { entry_type: :deposit, base_currency: 'USDT', base_amount: '150.2'.to_d, tx_id: 'dep-1', transacted_at: seen,
                raw_data: { 'amount' => '150.2', 'coin' => 'USDT', 'txId' => 'dep-1', 'insertTime' => (seen.to_f * 1000).to_i } }
    withdrawal = { entry_type: :withdrawal, base_currency: 'LTC', base_amount: '10.43980476'.to_d, fee_currency: 'LTC',
                   fee_amount: '0.001'.to_d, tx_id: 'wd-1', transacted_at: Time.utc(2021, 8, 23, 13, 8, 10),
                   raw_data: { 'amount' => '10.43980476', 'transactionFee' => '0.001', 'coin' => 'LTC', 'txId' => 'wd-1',
                               'applyTime' => '2021-08-23 13:08:10', 'completeTime' => '2021-08-23 13:20:44' } }
    AccountTransactionSync.new(@key).store!([deposit, withdrawal])
    file = [HEADER, '1,2021-07-01 14:07:04,Spot,Deposit,USDT,150.2,', '1,2021-08-23 13:20:44,Spot,Withdraw,LTC,-10.44080476,'].join("\n")

    result = import(file)

    assert_equal({ imported: 0, duplicates: 2 }, result.slice(:imported, :duplicates))
    assert_equal 2, AccountTransaction.for_user(@user).count
  end

  # A dust sweep: the API books the BNB gross with the charge beside it, the export writes it net, a
  # second later. Over a synced history, the file's conversion is recognised whole.
  test 'a file over a synced dust conversion adds none of it' do
    at = Time.utc(2021, 9, 21, 19, 22, 0)
    raw = { 'fromAsset' => 'USDT', 'amount' => '25.47403194', 'transferedAmount' => '0.07147454', 'serviceChargeAmount' => '0.00142949',
            'operateTime' => 1_632_252_120_000, 'transId' => 78_240_438_808, 'targetAsset' => 'BNB' }
    AccountTransactionSync.new(@key).store!(
      [{ entry_type: :swap_out, base_currency: 'USDT', base_amount: '25.47403194'.to_d, tx_id: 'dust-78240438808-out',
         group_id: 'dust_78240438808', transacted_at: at, raw_data: raw },
       { entry_type: :swap_in, base_currency: 'BNB', base_amount: '0.07147454'.to_d, fee_currency: 'BNB', fee_amount: '0.00142949'.to_d,
         tx_id: 'dust-78240438808-in', group_id: 'dust_78240438808', transacted_at: at, raw_data: raw }]
    )
    file = [HEADER, '1,2021-09-21 19:22:01,Spot,Small Assets Exchange BNB,USDT,-25.47403194,USDT to BNB',
            '1,2021-09-21 19:22:01,Spot,Small Assets Exchange BNB,BNB,0.07004505,USDT to BNB'].join("\n")

    result = import(file)

    assert_equal({ imported: 0, duplicates: 2 }, result.slice(:imported, :duplicates))
    assert_equal 2, AccountTransaction.for_user(@user).count
  end

  # The requirement, stated plainly.
  test 'importing the same file again changes nothing' do
    import

    result = import

    assert_equal 0, result[:imported]
    assert_equal 2, result[:duplicates]
    assert_equal 2, AccountTransaction.for_user(@user).count
  end

  # The case that actually matters: the file reaches back further than the API window, and overlaps
  # it. Only the part the API never had may land.
  test 'importing over what the API already synced adds only what it was missing' do
    AccountTransaction.create!(
      user: @user, api_key: @key, exchange: @binance, entry_type: :buy,
      base_currency: 'LTC', base_amount: 0.06983, quote_currency: 'USDT', quote_amount: 10.0003543,
      fee_currency: 'BNB', fee_amount: 0.00002482, tx_id: 'LTCUSDT-77-99',
      transacted_at: Time.utc(2021, 7, 1, 3, 42, 38)
    )

    result = import

    assert_equal 1, result[:imported], 'the deposit the API could not reach'
    assert_equal 1, result[:duplicates], 'the trade it already had, id or no id'
    assert_equal 'deposit', AccountTransaction.for_user(@user).order(:id).last.entry_type
  end

  # 182 such repeats in one real export; each is a credit of its own.
  test 'a file repeating a row in one second lands every copy, and once' do
    row = '1,2021-01-07 21:28:41,Spot,Commission History,USDT,0.06303219,'
    file = [HEADER, row, row].join("\n")

    assert_equal 2, import(file)[:imported]
    assert_equal({ imported: 0, duplicates: 2 }, import(file).slice(:imported, :duplicates))
  end

  test 'a token swap distribution is the dividend the API already synced' do
    AccountTransactionSync.new(@key).store!([{ entry_type: :airdrop, base_currency: 'USDT', base_amount: '118.55207'.to_d,
                                               tx_id: 'dividend-4050375634540573989',
                                               transacted_at: Time.utc(2024, 6, 26, 5, 26, 41), raw_data: {} }])

    result = import([HEADER, '1,2024-06-26 08:26:41,Spot,Token Swap - Distribution,USDT,118.55207,'].join("\n"), offset: '+03:00')

    assert_equal({ imported: 0, duplicates: 1 }, result.slice(:imported, :duplicates))
  end

  # Before it was read as an airdrop, the same row was stored as other income. Re-importing adds nothing.
  test 'a token swap distribution stored as other income by an older import is the same row' do
    AccountTransaction.create!(user: @user, api_key: @key, exchange: @binance, entry_type: :other_income, base_currency: 'USDT',
                               base_amount: '118.55207'.to_d, transacted_at: Time.utc(2024, 6, 26, 5, 26, 41),
                               description: 'Token Swap - Distribution', raw_data: { 'operation' => 'Token Swap - Distribution' })

    result = import([HEADER, '1,2024-06-26 08:26:41,Spot,Token Swap - Distribution,USDT,118.55207,'].join("\n"), offset: '+03:00')

    assert_equal({ imported: 0, duplicates: 1 }, result.slice(:imported, :duplicates))
  end

  # Why the zone is read from the file and never asked for: at the wrong offset nothing matches what
  # is already stored, so the same history lands a second time, hours out of place.
  test 'the same file at a different offset is a whole second history' do
    import

    assert_equal 0, import[:imported], 'the same offset recognises everything'
    assert_equal 2, import(offset: '+03:00')[:imported], 'a different one recognises nothing'
    assert_equal 4, AccountTransaction.for_user(@user).count
  end

  test 'the import does not move the sync watermark' do
    @key.update!(last_synced_at: Time.utc(2026, 1, 1))

    import

    assert_equal Time.utc(2026, 1, 1), @key.reload.last_synced_at,
                 'a file reaching back years must not tell the API it is caught up'
  end

  # ── our own format ───────────────────────────────────────────────────────────────────────────
  test 'what we export is what we can read back' do
    import
    exported = AccountTransaction.to_csv(AccountTransaction.for_user(@user))
    AccountTransaction.delete_all

    result = Import::Run.new(user: @user, api_key: @key, format: 'deltabadger', text: exported).import!

    assert_equal 2, result[:imported]
    assert_equal %w[buy deposit], AccountTransaction.for_user(@user).pluck(:entry_type).sort
    row = AccountTransaction.for_user(@user).find_by(entry_type: :buy)
    assert_equal 0.06983.to_d, row.base_amount
    assert_equal Time.utc(2021, 7, 1, 3, 42, 38), row.transacted_at
  end

  test 'our own export re-imported over itself changes nothing' do
    import
    exported = AccountTransaction.to_csv(AccountTransaction.for_user(@user))

    result = Import::Run.new(user: @user, api_key: @key, format: 'deltabadger', text: exported).import!

    assert_equal 0, result[:imported]
    assert_equal 2, AccountTransaction.for_user(@user).count
  end

  test 'an unreadable file is refused rather than half-imported' do
    result = Import::Run.new(user: @user, api_key: @key, format: 'binance',
                             text: "not,a,binance\nexport,at,all").import!

    assert result[:error].present?
    assert_equal 0, AccountTransaction.for_user(@user).count
  end

  # A Convert half with no partner in the window is not imported: stored alone it is an orphan the
  # complete export later cannot pair with, so it is reported and the rest of the file lands.
  test 'a convert half with no partner is reported, not imported' do
    file = [HEADER, '1,2021-07-01 14:07:04,Spot,Deposit,USDT,150.2,',
            '1,2025-06-22 20:20:00,Spot,Binance Convert,USDT,-129,',
            '1,2025-06-22 20:21:01,Spot,Binance Convert,USDC,129,'].join("\n")

    result = import(file)

    assert_nil result[:error]
    assert_equal 1, result[:imported]
    assert_equal %w[deposit], AccountTransaction.for_user(@user).pluck(:entry_type)
    assert_equal ['Binance Convert USDT -129 2025-06-22 20:20:00 UTC', 'Binance Convert USDC 129 2025-06-22 20:21:01 UTC'],
                 result[:unrecognised]
  end

  test 'a lone convert half, then the complete export, is one grouped pair' do
    half = [HEADER, '1,2025-06-22 20:20:45,Spot,Binance Convert,USDC,129,'].join("\n")
    whole = [HEADER, '1,2025-06-22 20:20:45,Spot,Binance Convert,USDC,129,', '1,2025-06-22 20:20:46,Spot,Binance Convert,USDT,-129,'].join("\n")

    import(half)
    result = import(whole)
    import(whole)

    assert_equal 2, result[:imported]
    rows = AccountTransaction.for_user(@user).order(:entry_type).to_a
    assert_equal %w[swap_in swap_out], rows.map(&:entry_type)
    assert_equal 1, rows.map(&:group_id).uniq.size
    assert rows.first.group_id.present?
  end

  # Time.parse fills what a cell lacks from the clock: an empty or garbled Time read as today, and the
  # row landed years from where it belongs. A cell that names no real date and time refuses the file.
  test 'a time cell that is no real date and time refuses the file' do
    ['', 'yesterday-ish', '2024-02-30 10:00:00', '2024-03-01 25:00:00', '2024-03-01'].each do |cell|
      file = [HEADER, '1,2021-07-01 14:07:04,Spot,Deposit,USDT,150.2,', "1,#{cell},Spot,Deposit,USDT,1,"].join("\n")

      assert_equal :malformed, import(file)[:error], cell.inspect
    end
    assert_equal 0, AccountTransaction.for_user(@user).count
  end

  test 'our own export with a date that names no day refuses the file' do
    header = AccountTransaction.csv_headers.join(',')
    file = "#{header}\n12:00:00,deposit,USDT,1,,,,,#{@binance.name_id},,,\n"

    assert_equal :malformed, import(file, format: 'deltabadger')[:error]
  end

  # `to_d` reads "1,5" as 1, "--1" as -0 and "Infinity" as infinite: a wrong number booked silently.
  test 'an amount that is no plain decimal refuses the file' do
    ['Infinity', '"1,5"', '--1', 'abc', '12abc', ''].each do |cell|
      file = [HEADER, '1,2021-07-01 14:07:04,Spot,Deposit,USDT,150.2,', "1,2021-07-02 00:00:00,Spot,Deposit,USDT,#{cell},"].join("\n")

      assert_equal :malformed, import(file)[:error], cell.inspect
    end
    assert_equal 0, AccountTransaction.for_user(@user).count
  end

  test 'plain decimals still read, exponent and sign included' do
    file = [HEADER, '1,2021-07-01 14:07:04,Spot,Deposit,USDT, 2.5 ,', '1,2021-07-01 14:07:05,Spot,Deposit,BTC,1e-5,',
            '1,2021-07-01 14:07:06,Spot,Deposit,ETH,+.5,'].join("\n")

    assert_equal 3, import(file)[:imported]
    assert_equal [2.5, 0.00001, 0.5].map(&:to_d), AccountTransaction.for_user(@user).order(:transacted_at).pluck(:base_amount)
  end

  test 'our own export with an amount that is no plain decimal refuses the file' do
    header = AccountTransaction.csv_headers.join(',')
    file = "#{header}\n2024-01-01T00:00:00Z,buy,ZORK,1,USD,12abc,,,#{@binance.name_id},,,\n"

    assert_equal :malformed, import(file, format: 'deltabadger')[:error]
  end

  # Same pair, same second: one price. Where Binance wrote one cash row for two fills (or one receipt
  # for two spends), the fills stay buy rows, so a file over the API's fills dedups against them.
  test 'a file whose cash leg spans two API-stored fills adds nothing' do
    at = Time.utc(2024, 3, 1, 10, 0, 0)
    fill = lambda { |id, eth, usdt|
      { entry_type: :buy, base_currency: 'ETH', base_amount: eth.to_d, quote_currency: 'USDT', quote_amount: usdt.to_d, tx_id: id, transacted_at: at,
        raw_data: {} }
    }
    AccountTransactionSync.new(@key).store!([fill.call('f1', '1', '10'), fill.call('f2', '2', '20')])

    one_spend = [HEADER, '1,2024-03-01 10:00:00,Spot,Transaction Spend,USDT,-30,', '1,2024-03-01 10:00:00,Spot,Transaction Buy,ETH,1,',
                 '1,2024-03-01 10:00:00,Spot,Transaction Buy,ETH,2,'].join("\n")
    two_spends = [HEADER, '1,2024-03-01 10:00:00,Spot,Transaction Spend,USDT,-10,', '1,2024-03-01 10:00:00,Spot,Transaction Spend,USDT,-20,',
                  '1,2024-03-01 10:00:00,Spot,Transaction Buy,ETH,3,'].join("\n")

    assert_equal 0, import(one_spend)[:imported]
    assert_equal 0, import(two_spends)[:imported]
    assert_equal 2, AccountTransaction.for_user(@user).count
  end
end
