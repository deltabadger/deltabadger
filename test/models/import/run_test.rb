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
end
