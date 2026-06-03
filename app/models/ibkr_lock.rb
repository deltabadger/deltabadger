# Cross-process, owner-safe, reentrant mutual exclusion for a single IBKR brokerage
# session. IBKR allows only one brokerage session per login, so every brokerage call
# for a given credential set must be serialized — across BOTH the web (Puma) and the
# jobs process (separate containers on Umbrel, sharing the primary SQLite file).
#
# The UNIQUE index on `key` is the atomic arbiter: acquiring = INSERT a row; a second
# INSERT for the same key fails with RecordNotUnique. `with_lock` is the only public API.
class IbkrLock < ApplicationRecord
  class Timeout < StandardError; end

  THREAD_KEY = :ibkr_lock_owners

  DEFAULT_TTL = 120  # seconds — comfortably above the worst-case (bounded) order/reply op
  DEFAULT_WAIT = 15  # seconds to wait for the lock before giving up
  POLL = 0.05        # backoff between acquire attempts

  class << self
    # Runs the block while holding the lock for `key`. Reentrant within the same thread
    # (a nested call for a key this thread already holds just yields). Raises Timeout if
    # the lock can't be acquired within `wait`.
    def with_lock(key, ttl: DEFAULT_TTL, wait: DEFAULT_WAIT)
      held = (Thread.current[THREAD_KEY] ||= {})
      return yield if held.key?(key) # already held by this thread — reentrant

      owner = acquire(key, ttl: ttl, wait: wait)
      held[key] = owner
      begin
        yield
      ensure
        held.delete(key)
        release(key, owner)
      end
    end

    # Returns the owner token on success, raises Timeout otherwise.
    def acquire(key, ttl: DEFAULT_TTL, wait: DEFAULT_WAIT)
      deadline = monotonic + wait
      owner = SecureRandom.uuid
      loop do
        reclaim_stale(key)
        create!(key: key, owner: owner, expires_at: Time.current + ttl)
        return owner
      rescue ActiveRecord::RecordNotUnique
        raise Timeout, "IBKR lock not acquired for #{key} within #{wait}s" if monotonic >= deadline

        sleep(POLL)
      rescue ActiveRecord::StatementInvalid => e
        raise unless sqlite_busy?(e)
        raise Timeout, "IBKR lock not acquired for #{key} within #{wait}s (db busy)" if monotonic >= deadline

        sleep(POLL)
      end
    end

    # Owner-safe: only removes the row if it is still the one this owner inserted, so a
    # holder whose TTL expired can never delete a newer holder's lock.
    def release(key, owner)
      where(key: key, owner: owner).delete_all
    end

    private

    def reclaim_stale(key)
      where(key: key).where('expires_at < ?', Time.current).delete_all
    end

    def sqlite_busy?(error)
      error.message.match?(/database is locked|database is busy/i)
    end

    def monotonic
      Process.clock_gettime(Process::CLOCK_MONOTONIC)
    end
  end
end
