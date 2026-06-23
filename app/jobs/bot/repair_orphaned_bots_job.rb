class Bot::RepairOrphanedBotsJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: 'RepairOrphanedBotsJob', on_conflict: :discard, duration: 15.minutes

  def perform
    repair_scheduled_and_retrying
    repair_wedged_waiting
  end

  private

  def repair_scheduled_and_retrying
    orphaned_bots = find_orphaned_bots
    return if orphaned_bots.empty?

    Rails.logger.info "[Bot Health] Found #{orphaned_bots.count} orphaned bot(s)"
    orphaned_bots.each do |bot|
      repair_bot(bot)
    rescue StandardError => e
      Rails.logger.error "[Bot Health] Failed to repair bot #{bot.id}: #{e.message}"
    end
  end

  # A :waiting limit bot is re-polled ONLY by its self-rescheduling *LimitCheckJob. If that job
  # raised and dead-lettered (e.g. an Alpaca data-API timeout before the Task-1 fix), the chain
  # is dead and the bot is stuck :waiting with no user alert. Detect "limited :waiting bot with
  # no pending check job" and re-enqueue the live check job — without changing status and without
  # touching bots that still have a queued check.
  def repair_wedged_waiting
    wedged = find_wedged_waiting_bots
    return if wedged.empty?

    Rails.logger.info "[Bot Health] Found #{wedged.count} wedged :waiting bot(s)"
    wedged.each do |bot|
      repair_waiting_bot(bot)
    rescue StandardError => e
      Rails.logger.error "[Bot Health] Failed to repair waiting bot #{bot.id}: #{e.message}"
    end
  end

  def find_orphaned_bots
    Bot.where(status: %i[scheduled retrying]).select do |bot|
      bot.exchange.present? && bot.next_action_job_at.nil?
    end
  end

  # A bot is wedged iff it has been :waiting STABLY (not a momentary mid-execute_action :waiting),
  # it is CURRENTLY limit-paused (`limited?` — latest limit_paused log is at/after the last
  # ActionJob run, so a stale historical pause doesn't count), and it has NO active limit-check job.
  # The WEDGE_GRACE staleness guard is a second line of defense: a bot running a NORMAL cycle that
  # is momentarily :waiting inside execute_action has no check job, so without the guard it could be
  # misread as wedged. A real wedge persists for many minutes; a normal :waiting resolves in
  # seconds, and updated_at moves on each status flip — so `updated_at < WEDGE_GRACE.ago` excludes
  # any bot still churning.
  WEDGE_GRACE = 2.minutes

  def find_wedged_waiting_bots
    Bot.where(status: :waiting).where(updated_at: ..WEDGE_GRACE.ago).select do |bot|
      bot.exchange.present? && bot.respond_to?(:limited?) && bot.limited? && !bot.pending_limit_check_job?
    end
  end

  def repair_bot(bot)
    Rails.logger.warn "[Bot Health] Repairing orphaned bot #{bot.id} (#{bot.class.name})"

    # Cancel any partially-stuck jobs
    bot.cancel_scheduled_action_jobs

    # Reschedule the bot
    Bot::ActionJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
    Bot::BroadcastAfterScheduledActionJob.perform_later(bot)

    Rails.logger.info "[Bot Health] Bot #{bot.id} rescheduled for #{bot.next_interval_checkpoint_at}"
  end

  def repair_waiting_bot(bot)
    # Re-confirm BOTH conditions right before enqueue (TOCTOU: a concurrent stop/execute or a
    # manual rescue may have moved the bot or re-armed the chain between selection and here). The
    # check job's own line-1 `return unless bot.waiting?` guard makes a stale enqueue harmless
    # anyway, but re-checking avoids a double-enqueue / dead weight.
    bot.reload
    return unless bot.waiting? && !bot.pending_limit_check_job?

    Rails.logger.warn "[Bot Health] Repairing wedged :waiting bot #{bot.id} (#{bot.class.name})"
    bot.enqueue_limit_check_job
    Rails.logger.info "[Bot Health] Bot #{bot.id} limit-check chain restarted"
  end
end
