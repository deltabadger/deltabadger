# Each venue's own trading lock, held from outside a job. Every job that trades for a bot — the tick, a
# rebalance, a sale, a redeploy — runs under one Solid Queue semaphore per exchange (BotJob, group
# Bot::ActionJob), and a job holds it from the moment it is dispatched to the moment it finishes. Taking
# that semaphore means no such job can be dispatched or claimed on the venue until it is handed back, and
# a job parked meanwhile loads its bot only after, so it sees whatever was committed under the lease. A
# status check alone would miss the window where a worker has claimed a tick but not yet written
# `executing`; a queue check alone would miss a tick that becomes due after the check. The semaphore
# misses neither, because Solid Queue itself will not let a tick become ready without it.
#
# Used by Bot::Merge and Bot::IndexSwitch — anything that rewrites a bot's composition or class.
module Bot::VenueLease
  # How long a venue is ours. Far above what a holder takes (a handful of DB writes), because the
  # queue's maintenance pass deletes an expired semaphore whoever holds it: a holder that outlives its
  # lease has lost exclusivity, and must not signal a semaphore a later holder may own.
  LEASE = 5.minutes

  # What SolidQueue::Semaphore reads off a job: the key the trading jobs compute
  # (concurrency group / key), a limit of one, and how long a lease a dead process leaves behind.
  ExchangeLease = Struct.new(:concurrency_key, :concurrency_limit, :concurrency_duration) do
    def self.for(exchange)
      new("Bot::ActionJob/exchange_#{exchange.name_id}", 1, LEASE)
    end
  end

  module_function

  # Yields while holding every venue's lease. A venue whose lease is taken means a trading job is
  # dispatched or running there: returns false without yielding, never races it. What was taken is
  # handed straight back, whichever way the block leaves.
  #
  # @param venues [Array<Exchange>]
  # @param holder [String] names the holder in the log line of an outlived lease
  # @return [Boolean] whether the block ran
  def hold(venues, holder:)
    leases = venues.uniq.sort_by(&:id).map { |venue| ExchangeLease.for(venue) }
    held = leases.take_while { |lease| SolidQueue::Semaphore.wait(lease) }
    leased_at = Time.current
    return false unless held.size == leases.size

    yield
    true
  ensure
    release(held, leased_at, holder) if held
  end

  # Hand the venues back, and let a tick parked meanwhile on each run now rather than at the
  # dispatcher's next maintenance pass. Unless the lease has run out: a semaphore may then already be
  # someone else's, and signalling it would let a third party in beside them. They expire on their own.
  def release(leases, leased_at, holder)
    if Time.current - leased_at >= LEASE
      Rails.logger.warn("#{holder} outlived its #{LEASE.inspect} lease; not signalling")
      return
    end

    leases.each do |lease|
      SolidQueue::Semaphore.signal(lease)
      SolidQueue::BlockedExecution.release_one(lease.concurrency_key)
    end
  end
end
