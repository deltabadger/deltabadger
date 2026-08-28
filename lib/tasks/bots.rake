namespace :bots do
  desc 'Convert any remaining pair bots into two-asset baskets now, rather than waiting for the job'
  task migrate_dual_to_multi: :environment do
    # Bot::ConvertDualAssetBotsJob does this on a schedule; this is the same pass on demand, for
    # when you have drained job processing and want the bots that were busy converted immediately.
    converted, skipped = Bot::DualToComposition.run!

    converted.each { |id| puts "converted bot #{id}" }
    skipped.each { |id, reason| puts "skipped bot #{id}: #{reason}" }

    strays = Bot::DualToComposition.sweep_stray_jobs!
    puts "repointed #{strays} stray job(s)" if strays.positive?
    puts "#{converted.size} converted, #{skipped.size} still on the old shape"
    puts 'Those convert on a later pass of Bot::ConvertDualAssetBotsJob.' if skipped.any?
  end
end
