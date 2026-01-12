Rails.application.configure do
  # Settings specified here will take precedence over those in config/application.rb.

  # Code is not reloaded between requests.
  config.cache_classes = true
  config.active_storage.service = :local
  # Eager load code on boot. This eager loads most of Rails and
  # your application in memory, allowing both threaded web servers
  # and those relying on copy on write to perform better.
  # Rake tasks automatically ignore this option for performance.
  config.eager_load = true

  # Full error reports are disabled and caching is turned on.
  config.consider_all_requests_local       = false
  config.action_controller.perform_caching = true
  config.cache_store = :solid_cache_store

  # Ensures that a master key has been made available in either ENV["RAILS_MASTER_KEY"]
  # or in config/master.key. This key is used to decrypt credentials (and other encrypted files).
  # config.require_master_key = true

  # Disable serving static files from the `/public` folder by default since
  # Apache or NGINX already handles this.
  config.public_file_server.enabled = ENV['RAILS_SERVE_STATIC_FILES'].present?

  # CSS is compiled and compressed by dartsass-rails, JS by esbuild
  # No additional Sprockets compression needed
  config.assets.css_compressor = nil

  # Do not fallback to assets pipeline if a precompiled asset is missed.
  config.assets.compile = false

  # `config.assets.precompile` and `config.assets.version` have moved to config/initializers/assets.rb

  # Enable serving of images, stylesheets, and JavaScripts from an asset server.
  # config.action_controller.asset_host = 'http://assets.example.com'

  # Specifies the header that your server uses for sending files.
  # config.action_dispatch.x_sendfile_header = 'X-Sendfile' # for Apache
  # config.action_dispatch.x_sendfile_header = 'X-Accel-Redirect' # for NGINX

  # Force all access to the app over SSL, use Strict-Transport-Security, and use secure cookies.
  # For self-hosted deployments, SSL can be disabled by not setting FORCE_SSL=true
  config.force_ssl = ENV['FORCE_SSL'] == 'true'

  # Use the lowest log level to ensure availability of diagnostic information
  # when problems arise.
  config.log_level = :info

  # Prepend all log lines with the following tags.
  config.log_tags = [ :request_id ]

  # Configure public file server for tests with Cache-Control for performance.
  config.public_file_server.headers = {
    'Cache-Control' => "public, max-age=#{1.year.to_i}"
  }

  # Use a real queuing backend for Active Job (and separate queues per environment)
  config.active_job.queue_adapter = :solid_queue
  config.solid_queue.connects_to = { database: { writing: :queue } }
  # config.active_job.queue_name_prefix = "deltabadger_#{Rails.env}"

  config.action_mailer.perform_caching = false

  # Ignore bad email addresses and do not raise email delivery errors.
  # Set this to true and configure the email server for immediate delivery to raise delivery errors.
  config.action_mailer.raise_delivery_errors = true

  # Enable locale fallbacks for I18n (makes lookups for any locale fall back to
  # the I18n.default_locale when a translation cannot be found).
  config.i18n.fallbacks = true

  # Send deprecation notices to registered listeners.
  config.active_support.deprecation = :notify

  # Use default logging formatter so that PID and timestamp are not suppressed.
  config.log_formatter = ::Logger::Formatter.new

  # Use a different logger for distributed setups.
  # require 'syslog/logger'
  # config.logger = ActiveSupport::TaggedLogging.new(Syslog::Logger.new 'app-name')

  if ENV["RAILS_LOG_TO_STDOUT"].present?
    logger           = ActiveSupport::Logger.new(STDOUT)
    logger.formatter = config.log_formatter
    config.logger    = ActiveSupport::TaggedLogging.new(logger)
  end

  # Do not dump schema after migrations.
  config.active_record.dump_schema_after_migration = false

  # Determine protocol from APP_ROOT_URL or use FORCE_SSL setting
  app_root_url = ENV.fetch('APP_ROOT_URL')
  default_protocol = app_root_url.start_with?('https://') ? 'https' : 'http'
  default_protocol = 'https' if ENV['FORCE_SSL'] == 'true'

  # Extract host from URL (remove protocol)
  app_host = app_root_url.gsub(/^https?:\/\//, '').gsub(/\/.*$/, '')

  config.action_mailer.default_url_options = { host: app_host, protocol: default_protocol }
  config.action_mailer.perform_deliveries = true
  config.action_mailer.delivery_method = :smtp
  config.action_mailer.smtp_settings = {
    address: ENV.fetch('SMTP_ADDRESS'),
    authentication: :plain,
    domain: ENV.fetch('SMTP_DOMAIN'),
    port: ENV.fetch('SMTP_PORT'),
    enable_starttsl_auto: true,
    user_name: ENV.fetch('SMTP_USER_NAME'),
    password: ENV.fetch('SMTP_PASSWORD')
  }
  routes.default_url_options = {host: app_host, protocol: default_protocol}

  # Host authorization configuration for self-hosted deployments
  if ENV['ALLOWED_HOSTS'].present?
    # Allow specific hosts from environment variable (comma-separated)
    # Example: ALLOWED_HOSTS=example.com,subdomain.example.com
    ENV['ALLOWED_HOSTS'].split(',').each do |host|
      config.hosts << host.strip
    end
    # Always allow localhost for local development
    config.hosts << "localhost"
    config.hosts << "127.0.0.1"
  else
    # For self-hosted deployments, allow all hosts if no specific hosts are configured
    # This provides flexibility for users running on their own domains/servers
    config.hosts.clear
  end

  config.exceptions_app = self.routes

  config.action_cable.allowed_request_origins = [ENV.fetch('APP_ROOT_URL')]
  config.action_cable.worker_pool_size = ENV.fetch('MAX_DB_CONNECTIONS', 4)

  config.dry_run = false
end
