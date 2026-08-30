module Bot::ActivityLoggable
  extend ActiveSupport::Concern

  included do
    has_many :bot_activity_logs, dependent: :destroy
  end

  # Append a durable lifecycle/decision event. Best-effort: a logging failure must
  # never propagate into the trading path.
  #
  # `at` is for an event the bot is being TOLD about rather than one it is doing — a corporate
  # action the venue booked months ago and a sync only just read. The feed orders by created_at,
  # so without it the line would sit at the top of the feed dated today, above the trades it
  # exists to explain.
  def log_activity(event, message = nil, level: :info, details: {}, at: nil)
    bot_activity_logs.create!(event:, message:, level:, details:, **(at ? { created_at: at } : {}))
  rescue StandardError => e
    Rails.logger.warn("log_activity failed bot_id=#{id} event=#{event} error=#{e.message}")
    nil
  end
end
