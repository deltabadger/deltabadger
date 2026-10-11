module Automation::Statusable
  extend ActiveSupport::Concern

  included do
    # Active Record saves already run in a write transaction. Fence ALL decorators and store
    # accessors here, including writes made before the tick claims :executing.
    before_save :require_captured_run, prepend: true
    after_save -> { capture_run! if @claimed_run }

    # `archived` is appended, never inserted: the column stores the position.
    enum :status, %i[created scheduled stopped deleted executing retrying waiting archived]

    scope :working, -> { where(status: %i[scheduled executing retrying waiting]) }
  end

  STOP_DISPATCH_NOTICE = 'Orders already sent to the exchange may still execute.'.freeze

  # What a fenced tick returns instead of a plain Success: it no longer owns the run, so neither the
  # bot (no :waiting) nor Bot::ActionJob (no :scheduled, no next job) may write anything after it.
  # A stop leaves the bot stopped; a restart's own tick and chain carry on untouched.
  FENCED = Result::Success.new({ fenced: true }.freeze).freeze

  def working?
    scheduled? || executing? || retrying? || waiting?
  end

  # A lifecycle generation changes even for a resumed Start in the same clock tick. The timestamp
  # and tick counter distinguish successive jobs within that generation. Capture this from the
  # job/instance BEFORE authentication, prices or polling; never adopt the current row at claim time.
  def run_identity
    [transient_data.fetch('run_generation', 0), transient_data.fetch('tick_generation', 0),
     self[:started_at]&.utc&.iso8601(6),
     last_action_job_at&.utc&.iso8601(6)]
  end

  def capture_run!(identity = run_identity)
    @claimed_run = identity
  end

  def owns_run?(working: true)
    self.class.uncached do
      current = self.class.find_by(id:)
      current && (!working || current.working?) && current.run_identity == @claimed_run
    end
  end

  def stopped_mid_tick?
    @claimed_run && !owns_run?
  end

  # Short database work only. Never put exchange I/O inside this block.
  def with_run_ownership(working: true)
    transaction do
      next FENCED if @claimed_run && !owns_run?(working:)

      yield
    end
  end

  def update_columns(attributes)
    return super unless @claimed_run

    with_run_ownership { super }
  end

  def retry_sqlite_busy
    yield
  rescue ActiveRecord::StatementInvalid => e
    raise unless e.cause.is_a?(SQLite3::BusyException)

    sleep 0.01
    retry
  end

  def require_captured_run
    throw(:abort) if @claimed_run && !owns_run?
  end

  def transition_working!(status)
    capture_run! unless @claimed_run
    result = with_run_ownership do
      if status.to_s == 'executing'
        current = self.class.uncached { self.class.find_by(id:) }
        next FENCED unless current&.scheduled? || current&.retrying?
      end
      update!(status:)
      true
    end
    !result.equal?(FENCED)
  end
end
