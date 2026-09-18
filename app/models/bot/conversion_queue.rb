# The queue side of converting a bot row from one class to another in place (Bot::DualToComposition,
# Bot::SingleToComposition). Solid Queue is a separate database, so none of this can join the conversion's
# transaction: it runs before it (is a job naming the bot in flight?) and after it (repoint the jobs).
#
# A job names its bot by GlobalID, which embeds the class — `gid://deltabadger/Bots::DcaSingleAsset/7`. After
# the type flips, a job naming the old class cannot find its row by that class, and the bot can neither see
# nor cancel it (Automation::Schedulable matches the GlobalID it computes now). So every job naming the old
# class is rewritten to the new one, which also keeps a scheduled tick's exact time.
#
# Only unfinished jobs are read: finished ones are history, and `finished_at` is indexed.
module Bot::ConversionQueue
  class Row < ActiveRecord::Base
    self.table_name = 'bots'
    self.inheritance_column = nil
  end

  # Running, dispatched, and waiting on the per-exchange semaphore. Scheduled is handled separately:
  # only a FUTURE scheduled execution is safe, and .due is exactly the set about to become Ready.
  BUSY_EXECUTIONS = %w[ClaimedExecution ReadyExecution BlockedExecution].freeze

  module_function

  # Whether a job naming any of these bots (`Class/id` fragments) is claimed, ready, blocked, or scheduled
  # and already due. A status check alone is not enough: Bot::ActionJob authenticates against the exchange
  # while the row still reads `scheduled`.
  def busy?(gids)
    return false unless defined?(SolidQueue)

    gids.any? do |gid|
      fragment = "%#{gid}\"%"
      BUSY_EXECUTIONS.any? do |execution|
        SolidQueue.const_get(execution).joins(:job).where('solid_queue_jobs.arguments LIKE ?', fragment).exists?
      end || SolidQueue::ScheduledExecution.due.joins(:job).where('solid_queue_jobs.arguments LIKE ?', fragment).exists?
    end
  end

  def repoint(bot_id, from:, to:)
    return 0 unless defined?(SolidQueue)

    old_gid = "#{from}/#{bot_id}"
    new_gid = "#{to}/#{bot_id}"
    unfinished.where('arguments LIKE ?', "%#{old_gid}\"%").find_each do |job|
      job.update_columns(arguments: rewrite_gids(job.arguments, old_gid, new_gid))
    end
  end

  # Unfinished jobs still naming `from` for a bot whose row is already `to`: a conversion interrupted
  # between its commit and its repoint, or a worker that re-enqueued under the old name after the flip.
  def sweep!(from:, to:)
    return 0 unless defined?(SolidQueue)

    converted_ids = Row.where(type: to).pluck(:id).to_set
    ids = unfinished.where('arguments LIKE ?', "%#{from}/%").filter_map do |job|
      job.arguments.to_s[%r{#{Regexp.escape(from)}/(\d+)}, 1]&.to_i
    end
    ids.uniq.select { |id| converted_ids.include?(id) }.each { |id| repoint(id, from:, to:) }.size
  end

  # Whether any unfinished job still names `from` at all.
  def names?(from)
    defined?(SolidQueue) && unfinished.where('arguments LIKE ?', "%#{from}/%").exists?
  end

  def unfinished = SolidQueue::Job.where(finished_at: nil)

  # Rebuilds rather than mutating: the parsed JSON may hold frozen strings, and sub! on one raises.
  # end_with? anchors the match so a pass for bot 7 can never rewrite bot 71's GlobalID.
  def rewrite_gids(node, old_gid, new_gid)
    case node
    when Hash   then node.transform_values { |value| rewrite_gids(value, old_gid, new_gid) }
    when Array  then node.map { |value| rewrite_gids(value, old_gid, new_gid) }
    when String then node.end_with?(old_gid) ? node.sub(old_gid, new_gid) : node
    else node
    end
  end
end
