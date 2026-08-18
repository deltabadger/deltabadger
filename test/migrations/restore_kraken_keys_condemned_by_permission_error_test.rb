require 'test_helper'
require Dir[Rails.root.join('db/migrate/*_restore_kraken_keys_condemned_by_permission_error.rb')].first

class RestoreKrakenKeysCondemnedByPermissionErrorTest < ActiveSupport::TestCase
  setup do
    ActiveRecord::Migration.verbose = false
    @user = create(:user)
    @kraken = create(:kraken_exchange)
  end

  def restore = RestoreKrakenKeysCondemnedByPermissionError.new.up

  def condemned_key(exchange:, error:)
    key = create(:api_key, user: create(:user), exchange: exchange)
    key.update_columns(status: ApiKey.statuses[:incorrect], last_sync_error: error)
    key
  end

  test 'a Kraken key condemned by the permission error is usable again' do
    key = condemned_key(exchange: @kraken, error: 'EGeneral:Permission denied')

    restore

    assert_equal 'correct', key.reload.status
  end

  # last_sync_error only exists since 20260815160000; the misclassification is far older, so the
  # users hurt longest are exactly the ones with nothing recorded.
  test 'a Kraken key condemned before errors were recorded is restored too' do
    key = condemned_key(exchange: @kraken, error: nil)

    restore

    assert_equal 'correct', key.reload.status
  end

  test 'a withdrawal key is out of scope — no sync job that condemns keys ever reads one' do
    key = create(:api_key, user: create(:user), exchange: @kraken, key_type: :withdrawal)
    key.update_columns(status: ApiKey.statuses[:incorrect], last_sync_error: nil)

    restore

    assert_equal 'incorrect', key.reload.status
  end

  test 'a Kraken key condemned for any other reason is left alone' do
    key = condemned_key(exchange: @kraken, error: 'EAPI:Invalid key')

    restore

    assert_equal 'incorrect', key.reload.status
  end

  # "Permission denied" is not Kraken-exclusive prose; on another venue it may well accompany a
  # genuinely dead key, and resurrecting that would send the user's bots at a revoked credential.
  test 'another exchange carrying the same message is left alone' do
    key = condemned_key(exchange: create(:binance_exchange), error: 'EGeneral:Permission denied')

    restore

    assert_equal 'incorrect', key.reload.status
  end

  test 'a healthy Kraken key is untouched' do
    key = create(:api_key, user: @user, exchange: @kraken)

    restore

    assert_equal 'correct', key.reload.status
  end
end
