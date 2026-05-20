module Bot::ActivityLoggable
  extend ActiveSupport::Concern

  included do
    has_many :bot_activity_logs, dependent: :destroy
  end

  # Append a durable lifecycle/decision event. Best-effort: a logging failure must
  # never propagate into the trading path.
  def log_activity(event, message = nil, level: :info, details: {})
    bot_activity_logs.create!(event:, message:, level:, details:)
  rescue StandardError => e
    Rails.logger.warn("log_activity failed bot_id=#{id} event=#{event} error=#{e.message}")
    nil
  end
end
