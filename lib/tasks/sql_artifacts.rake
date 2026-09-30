require 'tmpdir'

namespace :db do
  namespace :sql do
    # Boot against scratch databases: the runner boots Rails before the script switches connections.
    runner = lambda do |*args|
      Dir.mktmpdir do |boot|
        env = %w[DATABASE_PATH QUEUE_DATABASE_PATH CACHE_DATABASE_PATH CABLE_DATABASE_PATH]
              .each_with_index.to_h { |var, i| [var, "#{boot}/#{i}.sqlite3"] }
              .merge(%w[DATABASE_URL PRIMARY_DATABASE_URL QUEUE_DATABASE_URL CACHE_DATABASE_URL CABLE_DATABASE_URL]
                .to_h { |v| [v, nil] }) # a URL would override the paths
        sh(env, 'bin/rails', 'runner', 'script/sql_artifacts.rb', *args)
      end
    end

    desc 'Write db/sql baselines. The primary one is frozen at the Rust floor: FORCE=1 to overwrite it'
    task :baseline do
      primary = 'db/sql/primary_baseline.sql'
      if File.exist?(primary) && ENV['FORCE'] != '1'
        puts "#{primary} is frozen; new migrations need a twin in db/sql/migrate/"
      else
        runner.call('baseline_primary', primary)
      end
      Dir.mktmpdir do |dir|
        runner.call('generate', dir)
        %w[queue cache cable].each { |n| FileUtils.cp(File.join(dir, "#{n}_baseline.sql"), "db/sql/#{n}_baseline.sql") }
      end
    end

    desc 'Regenerate db/sql/seed.sql.gz from db/seeds.rb'
    task :seed do
      Dir.mktmpdir do |dir|
        runner.call('generate', dir)
        FileUtils.cp(File.join(dir, 'seed.sql.gz'), 'db/sql/seed.sql.gz')
      end
    end
  end
end
