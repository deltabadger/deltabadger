require Rails.root.join('lib/export_scratch')

class Export::Portable
  ENCRYPTED_POLICIES = {
    'ApiKey' => :clear, 'FeeApiKey' => :delete, 'AppConfig' => :filter,
    'User' => :keep, 'Rules::Withdrawal' => :keep
  }.freeze
  APP_CONFIG_KEYS = [
    AppConfig::COINGECKO_API_KEY, AppConfig::MARKET_DATA_TOKEN, AppConfig::MARKET_DATA_URL,
    AppConfig::MARKET_DATA_PROVIDER, AppConfig::PLATFORM_CONNECTED_AT,
    AppConfig::ALPACA_API_KEY, AppConfig::ALPACA_API_SECRET, AppConfig::ALPACA_MODE,
    AppConfig::SMTP_PROVIDER, AppConfig::SMTP_USERNAME, AppConfig::SMTP_PASSWORD, AppConfig::SMTP_HOST, AppConfig::SMTP_PORT
  ].freeze
  APP_CONFIG_PREFIXES = %w[proxy_ smtp_].freeze
  SESSION_TABLES = %w[
    oauth_access_grants oauth_access_tokens connected_clients oauth_applications
    action_mcp_session_messages action_mcp_session_subscriptions action_mcp_session_tasks action_mcp_sessions
  ].freeze

  def call
    ExportScratch.validate!
    validate_databases!
    output = Pathname.new(ExportScratch::OUTPUT)
    if output.symlink? || (output.exist? && (!output.directory? || !output.children.empty?))
      raise ExportScratch::UnsafeEnvironment, 'Export output directory must be empty and not a symlink'
    end

    begin
      FileUtils.mkdir_p([output.join('portable'), output.join('csv/bots')])
      migrate!
      scrub!
      write_databases(output.join('portable'))
      write_csv(output.join('csv'))
      manifest = {
        app_version: Rails.application.config.version,
        schema_version: ApplicationRecord.connection_pool.migration_context.current_version,
        files: output.glob('**/*').select(&:file?).map { |file| file.relative_path_from(output).to_s }.sort
      }
      output.join('manifest.json').write("#{JSON.pretty_generate(manifest)}\n")
    rescue StandardError
      FileUtils.rm_rf(output)
      raise
    end
  end

  private

  def validate_databases!
    @databases = %w[primary queue cache cable].zip(ExportScratch::DATABASE_FILES).to_h do |name, file|
      config = ActiveRecord::Base.configurations.configs_for(env_name: Rails.env, name: name)
      unless config&.adapter == 'sqlite3' && File.expand_path(config.database, Rails.root) == File.join(ExportScratch::STORAGE, file)
        raise ExportScratch::UnsafeEnvironment, 'Export databases must be the production SQLite files under /app/storage'
      end

      [name, config]
    end
  end

  def migrate!
    # Never inherit jobs or cached credentials from the source. Migrations may enqueue
    # new jobs, so prepare the queue first and leave its worker stopped throughout.
    @databases.except('primary').each do |name, config|
      ActiveRecord::Base.connection_handler.connection_pool_list.each do |pool|
        pool.disconnect! if pool.db_config.database == config.database
      end
      ['', '-wal', '-shm', '-journal'].each { |suffix| FileUtils.rm_f("#{config.database}#{suffix}") }
      ActiveRecord::Tasks::DatabaseTasks.with_temporary_connection(config) do
        ActiveRecord::Tasks::DatabaseTasks.load_schema(config, :ruby, Rails.root.join("db/#{name}_schema.rb"))
      end
    end
    Rake::Task['db:migrate:primary'].invoke
  end

  def scrub!
    ApplicationRecord.transaction do
      ENCRYPTED_POLICIES.each do |model_name, policy|
        model = model_name.constantize
        case policy
        when :clear
          columns = EncryptedCredentials::COVERAGE.fetch(model_name).fetch(:attributes).index_with(nil)
          model.update_all(columns.merge(status: :incorrect))
        when :delete
          model.delete_all
        when :filter
          keys = model.pluck(:key).select { |key| APP_CONFIG_KEYS.include?(key) || key.start_with?(*APP_CONFIG_PREFIXES) }
          model.where(key: keys).delete_all
        end
      end
      AppConfig.where(key: EngineLease::KEY).delete_all
      Bot.where.not(status: %i[deleted archived]).update_all(status: :stopped)
      Rule.where.not(status: %i[deleted archived]).update_all(status: :stopped)
      BotSignal.find_each { |signal| signal.update_columns(token: SecureRandom.hex(32), enabled: false) }
      User.update_all(reset_password_token: nil, confirmation_token: nil, remember_created_at: nil)
      SESSION_TABLES.each { |table| ApplicationRecord.connection.execute("DELETE FROM #{table}") }
    end
  end

  def write_databases(directory)
    # VACUUM INTO copies live rows, excluding deleted credentials in free pages and WALs.
    @databases.each_value do |config|
      ActiveRecord::Tasks::DatabaseTasks.with_temporary_connection(config) do |connection|
        destination = directory.join(File.basename(config.database)).to_s
        connection.execute("VACUUM INTO #{connection.quote(destination)}")
      end
    end
  end

  def write_csv(directory)
    directory.join('tracker.csv').write(AccountTransaction.to_csv(AccountTransaction.all))
    Bot.find_each do |bot|
      # Keep csv/bots/<id>-<label>.csv below 100 bytes even with a 64-bit ID,
      # so Docker can archive it without a PAX extended header.
      label = bot.label.to_s.parameterize.gsub(/[^a-z0-9-]/, '').first(50).presence || 'bot'
      directory.join('bots', "#{bot.id}-#{label}.csv").write(bot.orders_csv)
    end
    directory.join('README_CSV.txt').write(<<~TEXT)
      Exported by Deltabadger #{Rails.application.config.version}.
      Restore with this version or newer.

      tracker.csv is transaction interchange: it excludes manual prices, transfer links
      and raw data. Import it using the Deltabadger CSV format.
      bots/*.csv holds submitted, closed orders only, one file per bot.
      These files do not contain bot configuration.
    TEXT
  end
end
