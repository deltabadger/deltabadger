require 'test_helper'

# A reading key. Trade permission CONTAINS read permission, so `key_type` is a capability and not a
# category: `trading` and `read_only` sit on one axis where trading is the superset, and
# `withdrawal` is a scope of its own that never reads for us.
#
# What follows from that: the tracker asks for a reading key only when there is no working key to
# read with, a bot key satisfies it without anyone converting anything, and no venue needs a
# bespoke check — a key that can read is proven by reading.
class ReadOnlyApiKeyTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @binance = create(:binance_exchange)
    @kraken = create(:kraken_exchange)
  end

  # ── the subset relation ──────────────────────────────────────────────────────────────────────
  test 'a reading key reads, and so does a trading key' do
    reading = create(:api_key, user: @user, exchange: @binance, key_type: :read_only)
    trading = create(:api_key, user: @user, exchange: @kraken, key_type: :trading)

    assert_equal [reading, trading].sort_by(&:id), ApiKey.reading(@user.api_keys).sort_by(&:id)
  end

  test 'a withdrawal key never reads: a different scope, not a smaller one' do
    create(:api_key, user: @user, exchange: @binance, key_type: :withdrawal)

    assert_empty ApiKey.reading(@user.api_keys)
  end

  test 'a key the venue rejected reads nothing' do
    create(:api_key, user: @user, exchange: @binance, key_type: :read_only, status: :incorrect)

    assert_empty ApiKey.reading(@user.api_keys)
  end

  # ── one key per venue ────────────────────────────────────────────────────────────────────────
  # Load-bearing, not tidiness: AccountTransactionSync#duplicate? scopes id-less rows to the api_key
  # on purpose, so two keys on one account import every id-less row twice.
  test 'a venue holding both keys reads with the trading one' do
    trading = create(:api_key, user: @user, exchange: @binance, key_type: :trading)
    create(:api_key, user: @user, exchange: @binance, key_type: :read_only)

    assert_equal [trading], ApiKey.reading(@user.api_keys)
  end

  test 'a venue whose trading key was rejected falls back to the reading key' do
    create(:api_key, user: @user, exchange: @binance, key_type: :trading, status: :incorrect)
    reading = create(:api_key, user: @user, exchange: @binance, key_type: :read_only)

    assert_equal [reading], ApiKey.reading(@user.api_keys)
  end

  test 'one user does not read with another user\'s key' do
    mine = create(:api_key, user: @user, exchange: @binance, key_type: :read_only)
    create(:api_key, user: create(:user), exchange: @binance, key_type: :trading)

    assert_equal [mine], ApiKey.reading(@user.api_keys)
  end

  # ── bots never see it ────────────────────────────────────────────────────────────────────────
  test 'a reading key is invisible to the bots' do
    reading = create(:api_key, user: @user, exchange: @binance, key_type: :read_only)

    assert_empty ApiKey.for_bot(@user.id, @binance.id)
    assert_not reading.trading?
  end

  test 'deleting a reading key stops no bots: it was never trading with them' do
    reading = create(:api_key, user: @user, exchange: @binance, key_type: :read_only)
    bot = create(:dca_single_asset, user: @user, exchange: @binance, status: :scheduled)

    reading.stop_dependent_bots!

    assert_equal 'scheduled', bot.reload.status
  end

  # ── proven by reading ────────────────────────────────────────────────────────────────────────
  test 'a reading key is validated by reading, not by a permission report' do
    api_key = create(:api_key, user: @user, exchange: @binance, key_type: :read_only)
    @binance.expects(:set_client).with(api_key: api_key)
    @binance.expects(:get_balances).with(asset_ids: []).returns(Result::Success.new({}))
    api_key.stubs(:exchange).returns(@binance)

    assert api_key.get_validity.data
  end

  test 'a trading key is still validated against the trade permission' do
    api_key = create(:api_key, user: @user, exchange: @binance, key_type: :trading)
    @binance.expects(:get_api_key_validity).with(api_key: api_key).returns(Result::Success.new(true))
    api_key.stubs(:exchange).returns(@binance)

    assert api_key.get_validity.data
  end

  test 'credentials the venue names as bad are condemned' do
    api_key = create(:api_key, user: @user, exchange: @binance, key_type: :read_only)
    @binance.stubs(:set_client)
    @binance.stubs(:get_balances).returns(Result::Failure.new('API-key format invalid.'))
    api_key.stubs(:exchange).returns(@binance)

    result = api_key.get_validity

    assert result.success?
    assert_not result.data, 'the venue named the key, so the key is what is wrong'
  end

  # A venue that is down, rate-limiting or behind a proxy hiccup must not cost the user their key:
  # :incorrect drops it from every sync until new credentials are pasted, and there was never
  # anything wrong with these.
  test 'a venue that simply would not answer condemns nothing' do
    api_key = create(:api_key, user: @user, exchange: @binance, key_type: :read_only)
    @binance.stubs(:set_client)
    @binance.stubs(:get_balances).returns(Result::Failure.new('503 Service Unavailable'))
    api_key.stubs(:exchange).returns(@binance)

    result = api_key.get_validity

    assert result.failure?
    api_key.update_status!(result)
    assert api_key.reload.pending_validation?
  end
end
