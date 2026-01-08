class ApplicationJob < ActiveJob::Base
  # Automatically retry jobs that encounter transient errors
  # retry_on StandardError, wait: :polynomially_longer, attempts: 5

  # Use ActiveJob's built-in executions count for retry tracking
  def retry_count
    executions - 1
  end
end
