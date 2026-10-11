module Automation::Statusable
  extend ActiveSupport::Concern

  included do
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
     self[:started_at]&.iso8601(6),
     last_action_job_at&.iso8601(6)]
  end

  def capture_run!(identity = run_identity)
    @claimed_run = identity
  end

  def owns_run?
    self.class.uncached do
      current = self.class.find_by(id:)
      current&.working? && current.run_identity == @claimed_run
    end
  end

  def stopped_mid_tick?
    @claimed_run && !owns_run?
  end

  # SQLite BEGIN IMMEDIATE serializes this check and every write/dispatch inside the block with
  # STOP. In particular the venue call stays inside the transaction through its reply/exception.
  # STOP can return only after that call ends; an already transmitted request may still settle.
  def with_run_ownership
    transaction do
      next FENCED if @claimed_run && !owns_run?

      yield
    end
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
