# Shared start/stop/delete lifecycle for interval-scheduled bots
# (DcaSingleAsset, DcaIndex, DcaMultiAsset). Bots::Signal is passive (no
# scheduling) and keeps its own thin lifecycle instead of including this.
#
# Include LAST in the bot model so the prepended decorator chains
# (PriceLimitable & co. wrap #stop, SmartIntervalable wraps
# #effective_quote_amount) stay on top, exactly as they wrapped the former
# per-class definitions.
module Bot::Lifecycle
  extend ActiveSupport::Concern

  included do
    prepend(Module.new do
      def execute_action
        capture_run! unless instance_variable_defined?(:@claimed_run)
        return Automation::Statusable::FENCED if stopped_mid_tick?

        super
      end
    end)
  end

  def api_key_type
    :trading
  end

  def start(start_fresh: true)
    # Compute exactly once per call; pass the same value to decision, persistence, and wait_until.
    computed_start_at = start_fresh && start_time_enabled? ? initial_start_at : nil
    use_delayed_first = computed_start_at&.future?

    # call restarting_within_interval? before setting the status to :scheduled
    set_orders_now = !use_delayed_first && (start_fresh || !restarting_within_interval?)
    self.status = :scheduled
    self.stop_message_key = nil
    if use_delayed_first
      settings['start_at'] = computed_start_at.iso8601
      self.started_at = computed_start_at
      self.last_action_job_at = nil
      self.missed_quote_amount = nil
      set_missed_quote_amount # settings changed → Accountable requires this before save
    elsif start_fresh
      self.started_at = Time.current
      self.last_action_job_at = nil
      self.missed_quote_amount = nil
    end

    # Skip the automatic status bar broadcast if we're scheduling a delayed job,
    # since BroadcastAfterScheduledActionJob will handle it after the job is persisted
    @skip_status_bar_broadcast = !set_orders_now

    # One transaction for the check and the save: a Start that loaded this row before a delete, a
    # merge or a class switch (Bot::IndexSwitch) committed must not write :scheduled over it and arm
    # a tick. `self.class.where` is STI-scoped, so a row that is now another class reads as gone. The
    # adapter opens transactions as BEGIN IMMEDIATE, so the read here is serialised with that commit.
    #
    # A working bot is refused, read from the row rather than this copy: a stale tab's Start would
    # otherwise re-anchor the schedule and tick at once, placing an extra order. The REST API
    # answers the same case with 409 before it gets here.
    started = transaction do
      self.class.uncached do
        next false if persisted? && !self.class.where(id:).where.not(status: :deleted).exists?

        if persisted? && self.class.working.exists?(id)
          errors.add(:base, I18n.t('engine.already_running'))
          next false
        end

        # Read under the same write lock as the Start, including a resumed Start.
        current = self.class.find_by(id:)
        transient_data['run_generation'] = current&.transient_data&.fetch('run_generation', 0).to_i + 1
        valid?(:start) && save
      end
    end

    if started
      # Cancel any chain that is already live before arming a new one. Without this a start leaves
      # TWO chains for one bot, and because start_fresh also resets started_at — the anchor
      # next_interval_checkpoint_at computes the grid from — both chains reschedule off the SAME
      # new anchor, converge on the SAME grid point, and the bot places twice seconds apart.
      # Observed in production on a daily bot that traded cleanly at 13:52 for a week, was
      # restarted at 10:00:40, then placed two orders 26s apart at 10:00:43 the next day.
      #
      # Inside the success branch on purpose: cancelling on a failed start would silently stop a
      # bot the user never asked to stop. Before the enqueues on purpose: after them it would
      # destroy the job just created. Mirrors #stop (:68), Bot::Reversible (reversible.rb:116) and
      # Bot::RepairOrphanedBotsJob#repair_bot (repair_orphaned_bots_job.rb:67) — start was the only
      # re-enqueue site that did not do this.
      cancel_scheduled_action_jobs
      # Cancellation only touches older generations. If STOP/Start superseded this request,
      # leave its new chain alone; an enqueue that races this read still carries our old token.
      current = self.class.uncached { self.class.find_by(id:) }
      return true unless current&.working? && current.run_identity == run_identity

      if use_delayed_first
        Bot::ActionJob.set(wait_until: computed_start_at).perform_later(self)
        Bot::BroadcastAfterScheduledActionJob.perform_later(self)
      elsif set_orders_now
        Bot::ActionJob.perform_later(self)
      else
        Bot::ActionJob.set(wait_until: next_interval_checkpoint_at).perform_later(self)
        Bot::BroadcastAfterScheduledActionJob.perform_later(self)
      end
      log_activity('started', details: { start_fresh: start_fresh })
      true
    else
      false
    end
  end

  def stop(stop_message_key: nil)
    stopped = retry_sqlite_busy do
      transaction do
        # STOP owns only the stop fields. Reload under the dispatch lock so a stale request cannot
        # restore old transient data (including run identity) or resurrect an archived/deleted row.
        reload
        # Accountable's capture lives in instance variables, which reload does not reset. Its save
        # callback must not restore the pre-reload carry over money committed during dispatch.
        @carry_capture_pending = false
        next true if archived? || deleted?

        set_missed_quote_amount if settings_was != settings
        transient_data['run_generation'] = transient_data.fetch('run_generation', 0) + 1
        self.status = :stopped
        self.stopped_at = Time.current
        self.stop_message_key = stop_message_key
        # Unrelated invalid settings must never prevent STOP.
        next false unless save(validate: false)

        log_activity('stopped', details: { stop_message_key: }.compact)
        true
      end
    end
    cancel_scheduled_action_jobs if stopped
    stopped
  end

  def delete
    if update(
      status: 'deleted',
      stopped_at: Time.current
    )
      cancel_scheduled_action_jobs if exchange.present?
      true
    else
      false
    end
  end

  def restarting?
    stopped? && last_action_job_at.present?
  end

  def restarting_within_interval?
    return false unless restarting?

    # The buy carry (pending_quote_amount) is frozen while selling, so the buy-amount comparison
    # would be meaningless. Use elapsed time vs the (sell) cadence so a resumed selling bot does
    # not immediately re-sell mid-interval. try — Lifecycle is shared with non-reversible types.
    if try(:selling?)
      last_action_job_at.present? && (Time.current - last_action_job_at) < effective_interval_duration
    else
      pending_quote_amount < effective_quote_amount
    end
  end

  def effective_quote_amount
    quote_amount
  end

  private

  def action_job_config
    {
      queue: exchange.name_id,
      class: 'Bot::ActionJob',
      args: [{ '_aj_globalid' => to_global_id.to_s }]
    }
  end
end
