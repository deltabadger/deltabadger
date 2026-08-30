require 'test_helper'

# A cached Summary is a Marshal'd Data, so its MEMBER LIST is part of the payload. Add a member and
# every entry the previous build wrote becomes unreadable: Marshal raises TypeError, and Rails
# degrades only ArgumentError payloads to nil (Cache::Store#deserialize_entry rescues
# DeserializationError, which SerializerWithFallback raises for ArgumentError alone). So the key has
# to follow the shape on its own, and a read that lands on a shape it cannot parse has to come back
# cold rather than raise into the request that asked for it.
class Tracker::LedgerCacheShapeTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @user = create(:user)
    # The test env is :null_store; MemoryStore round-trips through Marshal, so a shape mismatch
    # fails here exactly as it does in production.
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
  end

  test 'the cache key moves when the summary gains a member' do
    before = Tracker::Ledger.send(:cache_key, @user, nil)

    with_summary_shape(Tracker::Ledger::Summary.members + [:extra]) do
      assert_not_equal before, Tracker::Ledger.send(:cache_key, @user, nil)
    end
  end

  test 'the cache key moves when a nested position gains a member' do
    before = Tracker::Ledger.send(:cache_key, @user, nil)

    with_shape(:Position, Tracker::Ledger::Position.members + [:extra]) do
      assert_not_equal before, Tracker::Ledger.send(:cache_key, @user, nil)
    end
  end

  test 'the cache key is stable across processes for one shape' do
    assert_equal Tracker::Ledger.send(:cache_key, @user, nil), Tracker::Ledger.send(:cache_key, @user, nil)
    assert_match(/\Atracker_ledger_v\d+_[0-9a-f]{8}_/, Tracker::Ledger.send(:cache_key, @user, nil))
  end

  test 'an entry written under an older shape reads as nil, not a raise' do
    key = Tracker::Ledger.send(:cache_key, @user, nil)
    with_summary_shape(%i[positions]) { Rails.cache.write(key, Tracker::Ledger::Summary.new(positions: [])) }

    assert_nil Tracker::Ledger.cached(@user)
  end

  private

  def with_summary_shape(members, &)
    with_shape(:Summary, members, &)
  end

  def with_shape(name, members)
    original = Tracker::Ledger.const_get(name)
    Tracker::Ledger.send(:remove_const, name)
    Tracker::Ledger.const_set(name, Data.define(*members))
    yield
  ensure
    Tracker::Ledger.send(:remove_const, name)
    Tracker::Ledger.const_set(name, original)
  end
end
