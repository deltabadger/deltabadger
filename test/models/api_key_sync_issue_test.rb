require 'test_helper'

# `sync_issue` speaks for the keys that READ. It used to ask `trading?`, which meant "not the
# withdrawal key" back when those were the only two kinds — and silently stopped meaning that when a
# third appeared. A reading key that cannot sync would have reported nothing at all, which is the
# one thing this method exists to prevent.
class ApiKeySyncIssueTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @binance = create(:binance_exchange)
  end

  def key(type, **attrs) = create(:api_key, user: @user, exchange: @binance, key_type: type, **attrs)

  test 'a reading key reports its failure' do
    issue = key(:read_only, last_sync_error: 'boom').sync_issue

    assert_equal :failed, issue[:reason]
    assert_equal 'Binance', issue[:exchange]
  end

  test 'a trading key still reports its failure' do
    assert_equal :failed, key(:trading, last_sync_error: 'boom').sync_issue[:reason]
  end

  # A withdrawal key never reads a ledger, so it has no history to be missing from.
  test 'a withdrawal key has nothing to say about history' do
    assert_nil key(:withdrawal, last_sync_error: 'boom').sync_issue
  end

  test 'a reading key that has never synced says so' do
    assert_equal :never_synced, key(:read_only, status: :correct, last_synced_at: nil).sync_issue[:reason]
  end
end
