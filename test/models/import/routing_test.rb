require 'test_helper'

# Which account a file's rows belong to — a question the FILE answers, not the user.
#
# Picking "Binance CSV" is picking Binance: the export exists nowhere else, and Binance.US writes
# the same one (it subclasses Binance), so those two are the entire universe of answers. Our own
# export names the venue on every row, so it routes itself across as many accounts as it covers.
#
# The user is asked exactly once — when they hold both Binance and Binance.US and the file cannot
# say which. Everywhere else a picker would be a control with one answer, and the wrong answer to it
# would file a Binance history under Hyperliquid.
class Import::RoutingTest < ActiveSupport::TestCase
  BINANCE = [
    'User ID,Time,Account,Operation,Coin,Change,Remark',
    '1,2021-07-01 14:07:04,Spot,Deposit,USDT,150.2,'
  ].join("\n").freeze

  setup do
    @user = create(:user)
    @binance = create(:binance_exchange)
    @binance_us = create(:binance_us_exchange)
    @hyperliquid = create(:hyperliquid_exchange)
  end

  # Hyperliquid validates its credential shape, so it gets a well-formed pair.
  def key_for(exchange)
    creds = if exchange.is_a?(Exchanges::Hyperliquid)
              { raw_key: "0x#{'1' * 40}", raw_secret: '0' * 64 }
            else
              {}
            end
    create(:api_key, user: @user, exchange: exchange, **creds)
  end

  # NOT `run`: Minitest::Runnable#import is the method that runs the test.
  def import(format: 'binance', text: BINANCE, api_key: nil)
    r = Import::Run.new(user: @user, format: format, text: text, offset: '+00:00', api_key: api_key)
    r.import!
  end

  # ── a venue format knows its venue ───────────────────────────────────────────────────────────
  test 'a Binance file finds the Binance account with nothing asked' do
    key = key_for(@binance)
    key_for(@hyperliquid)

    result = import

    assert_equal 1, result[:imported]
    assert_equal key, AccountTransaction.for_user(@user).sole.api_key
    assert_not result[:ambiguous], 'there was only ever one answer'
  end

  test 'a Binance.US account takes a Binance file too' do
    key = key_for(@binance_us)

    assert_equal 1, import[:imported]
    assert_equal key, AccountTransaction.for_user(@user).sole.api_key
  end

  # The point of the whole file.
  test 'a Binance file cannot be filed under Hyperliquid, however the request asks' do
    hyperliquid = key_for(@hyperliquid)

    result = import(api_key: hyperliquid)

    assert_equal 0, result[:imported]
    assert_equal :no_account_for_format, result[:error]
    assert_equal 0, AccountTransaction.for_user(@user).count
  end

  test 'holding both Binance accounts is the one case worth asking about' do
    key_for(@binance)
    key_for(@binance_us)

    result = import

    assert result[:ambiguous], 'the file cannot say which of the two it came from'
    assert_equal [@binance.id, @binance_us.id].sort, result[:candidates].map(&:exchange_id).sort
    assert_equal 0, AccountTransaction.for_user(@user).count, 'and nothing is written while it is unanswered'
  end

  test 'once asked, the answer is honoured — and only from the two it was offered' do
    key_for(@binance)
    us = key_for(@binance_us)

    assert_equal 1, import(api_key: us)[:imported]
    assert_equal us, AccountTransaction.for_user(@user).sole.api_key
  end

  # ── our own export routes itself ─────────────────────────────────────────────────────────────
  def deltabadger_csv(*rows)
    ([AccountTransaction.csv_headers.join(',')] + rows).join("\n")
  end

  test 'our export lands each row on the account its own column names' do
    binance = key_for(@binance)
    hyperliquid = key_for(@hyperliquid)

    result = import(
      format: 'deltabadger',
      text: deltabadger_csv(
        '2021-07-01T14:07:04Z,deposit,USDT,150.2,,,,,binance,tx-1,,',
        '2022-02-02T10:00:00Z,deposit,USDC,80,,,,,hyperliquid,tx-2,,'
      )
    )

    assert_equal 2, result[:imported]
    assert_equal binance, AccountTransaction.for_user(@user).find_by(tx_id: 'tx-1').api_key
    assert_equal hyperliquid, AccountTransaction.for_user(@user).find_by(tx_id: 'tx-2').api_key
  end

  # A file may name a venue this install has never connected. Those rows have no account to hang
  # from, so they are reported rather than quietly filed under whichever key happened to be first.
  test 'rows naming a venue with no account are reported, not rehomed' do
    key_for(@binance)

    result = import(
      format: 'deltabadger',
      text: deltabadger_csv(
        '2021-07-01T14:07:04Z,deposit,USDT,150.2,,,,,binance,tx-1,,',
        '2022-02-02T10:00:00Z,deposit,USDC,80,,,,,kraken,tx-2,,'
      )
    )

    assert_equal 1, result[:imported]
    assert_equal ['kraken'], result[:unrouted]
    assert_nil AccountTransaction.for_user(@user).find_by(tx_id: 'tx-2')
  end

  test 'a Binance file with no Binance account at all says which account is missing' do
    key_for(@hyperliquid)

    assert_equal :no_account_for_format, import[:error]
  end
end
