# Shared start/stop/delete lifecycle for interval-scheduled bots
# (DcaSingleAsset, DcaDualAsset, DcaIndex). Bots::Signal is passive (no
# scheduling) and keeps its own thin lifecycle instead of including this.
#
# Include LAST in the bot model so the prepended decorator chains
# (PriceLimitable & co. wrap #stop, SmartIntervalable wraps
# #effective_quote_amount) stay on top, exactly as they wrapped the former
# per-class definitions.
module Bot::Lifecycle
  extend ActiveSupport::Concern

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

    if valid?(:start) && save
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
      #
      # RESIDUAL, pre-existing and shared by every cancellation site: cancel_solid_queue_jobs
      # (schedulable.rb:105-133) reaches Scheduled, Ready and Blocked executions but NOT Claimed
      # ones. A job already running cannot be cancelled, so a stop-then-restart landing inside that
      # execution window leaves the in-flight job to finish against stale in-memory state and
      # reschedule itself — re-creating the second chain this call exists to prevent.
      # transition_working_bot! does not close it either: it excludes stopped/deleted, but a
      # restart has already flipped the row back to :scheduled. Closing it properly needs an
      # execution fence (a generation counter bumped here and re-checked before the job's final
      # writes), which is a schema change and deliberately not bolted on here.
      cancel_scheduled_action_jobs
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
    # A freshly loaded bot can carry recomputed settings defaults (after_initialize
    # concerns), which marks settings dirty — Accountable then requires
    # set_missed_quote_amount before any save (same guard start uses above).
    set_missed_quote_amount if settings_was != settings
    if update(
      status: :stopped,
      stopped_at: Time.current,
      stop_message_key:
    )
      cancel_scheduled_action_jobs
      log_activity('stopped', details: { stop_message_key: stop_message_key }.compact)
      true
    else
      false
    end
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
