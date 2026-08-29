# Finishes the pair-to-basket conversion the deploy-time migration could not.
#
# A bot that was executing or running a tick when the migration ran is left alone by design — and
# on an install nobody operates there is no one to drain job processing and retry. So the
# conversion recurs until nothing is left, and stops costing anything once the last
# pair bot is gone: one indexed count against `bots`.
#
# It also keeps running the two self-corrections a single pass cannot guarantee — undoing a stale
# instance's pair-shaped write, and repointing a GlobalID a worker re-enqueued under the old class
# name after the previous sweep had already passed.
#
# Removed with Bots::DcaDualAsset, once no install can still hold one.
class Bot::ConvertDualAssetBotsJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: 'ConvertDualAssetBotsJob', on_conflict: :discard, duration: 15.minutes

  def perform
    return unless Bot::DualToComposition.pending?

    converted, skipped = Bot::DualToComposition.run!
    Rails.logger.info("[dual→multi] converted #{converted.size}, #{skipped.size} left") if converted.any?
    skipped.each { |id, reason| Rails.logger.info("[dual→multi] bot #{id} waiting: #{reason}") }
  end
end
