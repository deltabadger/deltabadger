Rails.application.configure do
  # Settings specified here will take precedence over those in config/application.rb.

  # In the development environment your application's code is reloaded on
  # every request. This slows down response time but is perfect for development
  # since you don't have to restart the web server when you make code changes.
  config.cache_classes = false

  # Do not eager load code on boot.
# FIXME: Eager loading is enabled for Sidekiq because Kraken::Validator is not autoloaded. Review why this is happening: https://guides.rubyonrails.org/v5.2/autoloading_and_reloading_constants.html#autoloading-algorithms
# config.eager_load = false
  config.eager_load = Sidekiq.server?.present?

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
    config.cache_store = :file_store, Rails.root.join('tmp', 'cache', 'views')

    # DO NOT use the same REDIS_URL for caching and Sidekiq:
    # https://github.com/sidekiq/sidekiq/wiki/Using-Redis#multiple-redis-instances
    # https://medium.com/@simptive/rails-using-redis-for-caching-as-well-as-for-sidekiq-jobs-5254ba0d2f7d
    # config.cache_store = :redis_cache_store
  else
    config.action_controller.perform_caching = false
    config.cache_store = :null_store
  end

  # Don't care if the mailer can't send.
  config.action_mailer.raise_delivery_errors = true

  config.action_mailer.perform_caching = false

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
  config.action_mailer.default_url_options = { host: 'localhost', port: 3000 }

  routes.default_url_options = {host: ENV['APP_ROOT_URL'], protocol: 'https'}

end
