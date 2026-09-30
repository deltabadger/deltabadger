require 'tmpdir'

namespace :db do
  namespace :sql do
    # Boot against scratch databases: the runner boots Rails before the script switches connections.
    # <NAME>_DATABASE_URL overrides database.yml in every environment.
    runner = lambda do |*args|
      Dir.mktmpdir do |boot|
        env = %w[primary queue cache cable].to_h { |db| ["#{db.upcase}_DATABASE_URL", "sqlite3:#{boot}/#{db}.sqlite3"] }
        sh(env.merge('DATABASE_URL' => nil), 'bin/rails', 'runner', 'script/sql_artifacts.rb', *args)
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
