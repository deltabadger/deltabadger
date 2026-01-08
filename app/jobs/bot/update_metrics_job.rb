class Bot::UpdateMetricsJob < ApplicationJob
  queue_as :default

  # FIXME: ideally calling this job should kill any running Bot::UpdateMetricsJob
  # for the same bot, but by now we are only cancelling other enqueued jobs.

  def perform(bot)
    puts "Bot::UpdateMetricsJob.perform_later(#{bot.inspect})"
    cancel_other_update_metrics_jobs(bot)
    bot.metrics(force: true)
    Bot::BroadcastMetricsUpdateJob.perform_later(bot)
  end

  private

  def cancel_other_update_metrics_jobs(bot)
    return unless defined?(SolidQueue)

    global_id = bot.to_global_id.to_s

    # Cancel scheduled jobs
    SolidQueue::ScheduledExecution.joins(:job)
      .where(solid_queue_jobs: { class_name: 'Bot::UpdateMetricsJob' })
      .find_each do |execution|
        execution.job.destroy if job_matches_record?(execution.job, global_id)
      end

    # Cancel ready (queued) jobs
    SolidQueue::ReadyExecution.joins(:job)
      .where(solid_queue_jobs: { class_name: 'Bot::UpdateMetricsJob' })
      .find_each do |execution|
        execution.job.destroy if job_matches_record?(execution.job, global_id)
      end
  end

  def job_matches_record?(job, global_id)
    return false unless job&.arguments.present?

    args = job.arguments
    args.any? do |arg|
      (arg.is_a?(Hash) && arg['_aj_globalid'] == global_id) ||
      (arg.is_a?(String) && arg == global_id)
    end
  end
end
