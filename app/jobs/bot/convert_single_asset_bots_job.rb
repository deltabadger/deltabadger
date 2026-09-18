# Finishes the single-asset to multi-asset conversion the deploy-time migration could not: a bot that was
# mid-tick or held a job in flight is left alone by design, and an install nobody operates has no one to
# retry it. So it recurs, and costs a few indexed reads once nothing is left.
#
# It also keeps running the two self-corrections a single pass cannot guarantee — undoing a stale
# instance's single-asset write, and repointing a GlobalID a worker re-enqueued under the old class name.
#
# Removed with Bots::DcaSingleAsset, once no install can still hold one.
class Bot::ConvertSingleAssetBotsJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: 'ConvertSingleAssetBotsJob', on_conflict: :discard, duration: 15.minutes

  def perform
    return unless Bot::SingleToComposition.pending?

    converted, skipped = Bot::SingleToComposition.run!
    Rails.logger.info("[single→multi] converted #{converted.size}, #{skipped.size} left") if converted.any?
    skipped.each { |id, reason| Rails.logger.info("[single→multi] #{id} waiting: #{reason}") }
  end
end
