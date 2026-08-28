namespace :bots do
  desc 'Convert any remaining pair bots into two-asset baskets'
  task migrate_dual_to_multi: :environment do
    converted, skipped = Bot::DualToComposition.run!

    converted.each { |id| puts "converted bot #{id}" }
    skipped.each { |id, reason| puts "skipped bot #{id}: #{reason}" }

    # run! sweeps stray GlobalIDs itself; this second pass catches anything a worker re-enqueued
    # under the old class name while the run was in progress.
    strays = Bot::DualToComposition.sweep_stray_jobs!
    puts "repointed #{strays} stray job(s)" if strays.positive?
    puts "#{converted.size} converted, #{skipped.size} still on the old shape"
  end
end
