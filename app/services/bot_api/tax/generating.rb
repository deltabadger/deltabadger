# frozen_string_literal: true

module BotApi
  module Tax
    # Tax::GenerateReportJob limits itself to one crypto report per user and DISCARDS a second
    # enqueue (limits_concurrency … on_conflict: :discard). A queued or running one is visible
    # as an unfinished Solid Queue job under that key — asked for through the job's own
    # concurrency_key so the format is never guessed here.
    module Generating
      # A failed run also stays finished_at: nil (its row moves to failed_executions), and it
      # holds no slot — counting it would pin every later request on one bad report.
      def self.for?(user)
        key = ::Tax::GenerateReportJob.new(user.id, 'XX', 0).concurrency_key
        SolidQueue::Job.where(class_name: ::Tax::GenerateReportJob.name, concurrency_key: key, finished_at: nil)
                       .where.not(id: SolidQueue::FailedExecution.select(:job_id))
                       .exists?
      end

      # ActiveJob sets successfully_enqueued? unconditionally once the adapter returns
      # (ActiveJob::Enqueuing#_raw_enqueue), overwriting what Solid Queue decided — a job the
      # concurrency control discarded still reports true. The row's survival is the real answer.
      def self.accepted?(job)
        # perform_later returns false outright when an :enqueue callback aborts.
        return false unless job.respond_to?(:successfully_enqueued?) && job.successfully_enqueued?

        id = job.provider_job_id
        id.nil? || SolidQueue::Job.exists?(id: id)
      end
    end
  end
end
