namespace :bots do
  desc 'Convert any remaining pair bots into two-asset baskets (run with job processing drained)'
  task migrate_dual_to_multi: :environment do
    # defer_scheduled: false — the deploy-time migration refuses any bot with a queued job, because
    # a live worker could fire it between the type flip and the queue repoint. This task is the
    # answer to that: run it with bot job processing drained and the parked schedules convert too.
    puts 'Convert with bot job processing drained — a running worker can fire a tick mid-conversion.'
    converted, skipped = Bot::DualToComposition.run!(defer_scheduled: false)

    converted.each { |id| puts "converted bot #{id}" }
    skipped.each { |id, reason| puts "skipped bot #{id}: #{reason}" }

    # run! sweeps stray GlobalIDs itself; this second pass catches anything re-enqueued under the
    # old class name while the run was in progress.
    strays = Bot::DualToComposition.sweep_stray_jobs!
    puts "repointed #{strays} stray job(s)" if strays.positive?
    puts "#{converted.size} converted, #{skipped.size} still on the old shape"
  end
end
