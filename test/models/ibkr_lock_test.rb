require 'test_helper'

class IbkrLockTest < ActiveSupport::TestCase
  # The lock relies on COMMITTED rows being visible across DB connections/threads,
  # so this suite cannot run inside the per-test transaction.
  self.use_transactional_tests = false

  teardown do
    IbkrLock.delete_all
    Thread.current[:ibkr_lock_owners] = nil
  end

  test 'with_lock returns the block value' do
    assert_equal 42, IbkrLock.with_lock('k_value') { 42 }
  end

  test 'releases the row after the block (no leak)' do
    IbkrLock.with_lock('k_rel') { :ok }
    assert_equal 0, IbkrLock.where(key: 'k_rel').count
    refute Thread.current[:ibkr_lock_owners]&.key?('k_rel')
  end

  test 'releases the lock even when the block raises' do
    assert_raises(RuntimeError) { IbkrLock.with_lock('k_raise') { raise 'boom' } }
    assert_equal 0, IbkrLock.where(key: 'k_raise').count
    refute Thread.current[:ibkr_lock_owners]&.key?('k_raise')
  end

  test 'is reentrant within the same thread (nested same-key yields, single row)' do
    result = IbkrLock.with_lock('k_re') do
      inner = IbkrLock.with_lock('k_re') { :inner }
      assert_equal 1, IbkrLock.where(key: 'k_re').count, 'reentrant call must not create a second row'
      inner
    end
    assert_equal :inner, result
    assert_equal 0, IbkrLock.where(key: 'k_re').count
  end

  test 'serializes across threads: a second holder cannot take a held key and times out' do
    holding = Queue.new
    keep_holding = Queue.new

    holder = Thread.new do
      IbkrLock.with_lock('k_excl', ttl: 120, wait: 1) do
        holding.push(:held)
        keep_holding.pop # block inside the critical section until released
      end
    ensure
      ActiveRecord::Base.connection_pool.release_connection
    end

    holding.pop # ensure the holder is inside the lock
    assert_raises(IbkrLock::Timeout) do
      IbkrLock.with_lock('k_excl', ttl: 120, wait: 0.3) { flunk 'must not enter while held' }
    end

    keep_holding.push(:go)
    holder.join

    # now free
    assert_equal :reacquired, IbkrLock.with_lock('k_excl', ttl: 120, wait: 1) { :reacquired }
  end

  test 'reclaims an expired lock' do
    IbkrLock.create!(key: 'k_stale', owner: 'old', expires_at: 1.second.ago)
    assert_equal :got, IbkrLock.with_lock('k_stale', ttl: 120, wait: 1) { :got }
  end

  test 'release only deletes the row owned by the given owner' do
    IbkrLock.create!(key: 'k_owner', owner: 'owner-A', expires_at: 1.hour.from_now)

    IbkrLock.release('k_owner', 'someone-else')
    assert_equal 1, IbkrLock.where(key: 'k_owner').count, 'must not delete a row owned by another holder'

    IbkrLock.release('k_owner', 'owner-A')
    assert_equal 0, IbkrLock.where(key: 'k_owner').count
  end
end
