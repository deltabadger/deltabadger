require 'test_helper'

class BotActivityLogPruneJobTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  test 'deletes activity logs older than 90 days and keeps newer ones' do
    bot = create(:dca_single_asset)
    old = bot.bot_activity_logs.create!(event: 'started', created_at: 91.days.ago)
    recent = bot.bot_activity_logs.create!(event: 'started', created_at: 1.day.ago)

    BotActivityLog::PruneJob.new.perform

    assert_not BotActivityLog.exists?(old.id)
    assert BotActivityLog.exists?(recent.id)
  end
end
