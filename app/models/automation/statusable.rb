module Automation::Statusable
  extend ActiveSupport::Concern

  included do
    # `archived` is appended, never inserted: the column stores the position.
    enum :status, %i[created scheduled stopped deleted executing retrying waiting archived]

    scope :working, -> { where(status: %i[scheduled executing retrying waiting]) }
  end

  def working?
    scheduled? || executing? || retrying? || waiting?
  end

  # The tick's placement fence, read right before each order is sent. A stop, archive or delete can
  # commit while a claimed tick (:executing in this copy) sizes its orders, and a job already running
  # cannot be cancelled, so the tick asks the row, not this copy. Status alone is not enough: a stop
  # and a start put the row back to working, and the restart's own tick may claim it again. So the
  # row must still be :executing under the run this tick claimed (#claimed_run). A stop that commits
  # after this read still lets that one order go; the next leg is fenced. Only a claimed tick is
  # fenced: nothing else places through the tick's order setters.
  def stopped_mid_tick?
    return false unless executing?

    run = claimed_run
    run.nil? || run != @claimed_run
  end

  # A tick's own status write, made only while the bot is still working. A stop is a separate request
  # that can land while the tick waits on the exchange; a plain update! would undo it and the bot
  # would keep trading. The check and the write share one IMMEDIATE transaction, so a stop committed
  # before it always wins. Returns whether the bot moved.
  def transition_working!(status)
    transaction do
      next false unless self.class.uncached { self.class.working.exists?(id) }

      update!(status:)
      @claimed_run = claimed_run if executing?
      true
    end
  end

  private

  # What tells one claimed run from the next: a fresh start resets started_at, and every tick stamps
  # last_action_job_at before it claims the bot (Bot::ActionJob#perform). Read from the row both
  # times, so the two reads compare like for like. nil once the row is no longer :executing.
  # Uncached, as is the check in #transition_working!: a job runs under the query cache, and a stop
  # committed through another connection clears nothing here, so the same query would answer from
  # the claim.
  def claimed_run
    self.class.uncached do
      self.class.where(id:, status: :executing)
          .pick(:started_at, Arel.sql("json_extract(transient_data, '$.last_action_job_at')"))
    end
  end
end
