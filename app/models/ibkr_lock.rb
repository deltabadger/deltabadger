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

  # Above the worst-case held op: a place_order POST + up to 5 confirmation replies + a one-shot
  # 401 self-heal (re-mint LST + ssodh/init), each bounded by the ~30s read timeout. A crashed
  # holder blocks others only until this TTL; live callers give up after DEFAULT_WAIT and retry.
  DEFAULT_TTL = 300  # seconds
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

    # Owner-safe: only removes the row if it is still the one this owner inserted, so a holder
    # whose TTL expired can never delete a newer holder's lock. Must never raise (it runs in an
    # ensure block) — a busy DB is retried briefly, then left for the TTL to reclaim.
    def release(key, owner)
      attempts = 0
      begin
        where(key: key, owner: owner).delete_all
      rescue ActiveRecord::StatementInvalid => e
        raise unless sqlite_busy?(e)

        attempts += 1
        if attempts < 3
          sleep(POLL)
          retry
        end
        Rails.logger.warn("[IbkrLock] release busy for #{key}, leaving it to expire: #{e.message}")
      rescue StandardError => e
        Rails.logger.warn("[IbkrLock] release failed for #{key}, leaving it to expire: #{e.message}")
      end
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
