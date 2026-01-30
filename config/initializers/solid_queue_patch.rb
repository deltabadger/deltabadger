# frozen_string_literal: true

# Workaround for https://github.com/rails/solid_queue/issues/699
# Bug: `failed_with` uses `create_or_find_by!` with `exception:` param,
# but the actual DB column is `error`. When find_by triggers, it passes
# the exception object to type casting, causing TypeError.
#
# Remove this file after upgrading to SolidQueue > 1.3.1 with the fix.
Rails.application.config.after_initialize do
  next unless defined?(SolidQueue) && defined?(SolidQueue::VERSION)
  next unless SolidQueue::VERSION == "1.3.1"

  Rails.logger.info "Applying SolidQueue ProcessMissingError patch (issue #699)"

  module SolidQueueRetryablePatch
    def failed_with(exception)
      SolidQueue::FailedExecution.create!(job_id: id, exception: exception)
    end
  end

  SolidQueue::Job.prepend(SolidQueueRetryablePatch)
end
