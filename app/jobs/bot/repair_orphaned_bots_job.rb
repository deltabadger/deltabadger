class Bot::RepairOrphanedBotsJob < ApplicationJob
  queue_as :low_priority

  def perform
    orphaned_bots = find_orphaned_bots
    return if orphaned_bots.empty?

    Rails.logger.info "[Bot Health] Found #{orphaned_bots.count} orphaned bot(s)"

    orphaned_bots.each do |bot|
      repair_bot(bot)
    rescue StandardError => e
      Rails.logger.error "[Bot Health] Failed to repair bot #{bot.id}: #{e.message}"
    end
  end

  private

  def find_orphaned_bots
    Bot.where(status: [:scheduled, :retrying]).select do |bot|
      bot.exchange.present? && bot.next_action_job_at.nil?
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
end
