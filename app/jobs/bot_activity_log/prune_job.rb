class BotActivityLog::PruneJob < ApplicationJob
  RETENTION = 90.days

  def perform
    BotActivityLog.where('created_at < ?', RETENTION.ago).delete_all
  end
end
