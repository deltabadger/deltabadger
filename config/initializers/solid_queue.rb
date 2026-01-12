# Solid Queue configuration
Rails.application.config.solid_queue.connects_to = { database: { writing: :queue } }

# Log to STDOUT in development for visibility
if Rails.env.development?
  Rails.application.config.solid_queue.logger = ActiveSupport::Logger.new($stdout)
end

# Clean up stale processes on startup (helpful for development restarts)
# This runs after Rails is fully initialized
Rails.application.config.after_initialize do
  if Rails.env.development?
    Rails.application.executor.wrap do
      begin
        # Remove processes that haven't sent a heartbeat in the last 2 minutes
        # These are likely orphaned from abrupt app restarts
        stale_threshold = 2.minutes.ago
        stale_count = SolidQueue::Process.where("last_heartbeat_at < ?", stale_threshold).delete_all

        if stale_count > 0
          Rails.logger.info "[SolidQueue] Cleaned up #{stale_count} stale process(es) from previous run"
        end

        # Also clean up any claimed executions from dead processes
        # This releases jobs that were being processed when the app was killed
        orphaned_executions = SolidQueue::ClaimedExecution
          .joins("LEFT JOIN solid_queue_processes ON solid_queue_processes.id = solid_queue_claimed_executions.process_id")
          .where(solid_queue_processes: { id: nil })

        orphaned_count = orphaned_executions.count
        if orphaned_count > 0
          orphaned_executions.find_each(&:release!)
          Rails.logger.info "[SolidQueue] Released #{orphaned_count} orphaned job(s) back to queue"
        end

        # Auto-retry jobs that failed due to dead process pruning
        # These are safe to retry as they never actually ran
        pruned_failures = SolidQueue::FailedExecution.where("error LIKE ?", "%Process was found dead and pruned%")
        pruned_count = pruned_failures.count
        if pruned_count > 0
          pruned_failures.find_each(&:retry)
          Rails.logger.info "[SolidQueue] Auto-retried #{pruned_count} job(s) that failed due to dead process"
        end
      rescue => e
        # Don't prevent app startup if cleanup fails
        Rails.logger.warn "[SolidQueue] Cleanup on startup failed: #{e.message}"
      end
    end
  end
end
