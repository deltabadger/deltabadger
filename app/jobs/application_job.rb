class ApplicationJob < ActiveJob::Base
  # Automatically retry jobs that encounter transient errors
  # retry_on StandardError, wait: :polynomially_longer, attempts: 5

  # Run every job in the app zone, whatever the payload says.
  #
  # ActiveJob stamps Time.zone into the payload at enqueue and re-applies it around perform. A
  # self-rescheduling chain (Bot::ActionJob) enqueues its successor from inside that block, so it
  # re-stamps the same zone on every hop: one enqueue under a foreign zone pins it on the chain
  # for as long as the chain lives — months, and invisibly, since nothing in the app sets
  # Time.zone any more. Under a DST zone, calendar math then silently shifts an interval grid by
  # the offset. Registered after ActiveJob's own zone callback, so it runs inside it and wins.
  # Views that want the user's zone ask for it explicitly (in_time_zone), so nothing needs the
  # inherited one.
  around_perform { |_job, block| Time.use_zone(Time.zone_default, &block) }

  # And clean the payload itself, not just the perform window. retry_on handlers — and the retry
  # enqueue they trigger — run OUTSIDE the perform callbacks (rescue_with_handler is called once
  # run_callbacks has unwound), and serialize reads this attribute rather than the current zone,
  # so a retried job would carry a stale zone onward. Normalizing here also disarms every payload
  # already sitting in a queue: the first hop after deploy drops the foreign zone for good.
  def deserialize(job_data)
    super
    self.timezone = Time.zone_default.name
  end

  # Use ActiveJob's built-in executions count for retry tracking
  def retry_count
    executions - 1
  end
end
