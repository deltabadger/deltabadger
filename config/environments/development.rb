Rails.application.configure do  
  config.after_initialize do
    # Bullet.enable        = true
    # Bullet.alert         = false
    # Bullet.bullet_logger = true
    # Bullet.console       = true
    # Bullet.rails_logger  = true
    # Bullet.add_footer    = true

    Rack::MiniProfiler.config.start_hidden = true
  end

  # Settings specified here will take precedence over those in config/application.rb.

  # In the development environment your application's code is reloaded on
  # every request. This slows down response time but is perfect for development
  # since you don't have to restart the web server when you make code changes.
  config.enable_reloading = true

  # Do not eager load code on boot.
  config.eager_load = false

  # Show full error reports.
  config.consider_all_requests_local = true

  # Configure public file server for tests with Cache-Control for performance.
  config.public_file_server.headers = {
    'Cache-Control' => "public, max-age=#{2.days.to_i}"
  }

  # Enable/disable caching. By default caching is disabled.
  # Run rails dev:cache to toggle caching.
  if Rails.root.join('tmp', 'caching-dev.txt').exist?
    config.action_controller.perform_caching = true
    config.cache_store = :solid_cache_store
  else
    config.action_controller.perform_caching = false
    config.cache_store = :null_store
  end

  # Don't care if the mailer can't send.
  config.action_mailer.raise_delivery_errors = true

  config.action_mailer.perform_caching = false

  # Configure mailer preview path
  config.action_mailer.preview_paths << Rails.root.join("test/mailers/previews")

  # Print deprecation notices to the Rails logger.
  config.active_support.deprecation = :log

  # Raise an error on page load if there are pending migrations.
  config.active_record.migration_error = :page_load

  # Highlight code that triggered database queries in logs.
  config.active_record.verbose_query_logs = true

  # Debug mode disables concatenation and preprocessing of assets.
  # This option may cause significant delays in view rendering with a large
  # number of complex assets.
  config.assets.debug = true

  config.assets.digest = false

  # Suppress logger output for asset requests.
  config.assets.quiet = true

  config.active_storage.service = :local
  # Raises error for missing translations
  # config.action_view.raise_on_missing_translations = true

  # Use an evented file watcher to asynchronously detect changes in source code,
  # routes, locales, etc. This feature depends on the listen gem.
  config.file_watcher = ActiveSupport::EventedFileUpdateChecker

  config.action_mailer.delivery_method = :letter_opener
  config.action_mailer.perform_deliveries = true
  config.action_mailer.default_url_options = { host: ENV.fetch('APP_ROOT_URL'), protocol: 'http' }
  config.action_mailer.asset_host = "http://#{ENV.fetch('APP_ROOT_URL')}"

  routes.default_url_options = { host: ENV.fetch('APP_ROOT_URL'), protocol: 'http' }

  config.dry_run = %w[true 1 yes].include?(ENV.fetch('DRY_RUN', 'false').to_s.downcase)

end
