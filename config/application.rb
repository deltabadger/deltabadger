require_relative 'boot'

require "rails"
# Pick the frameworks you want:
require "active_model/railtie"
require "active_job/railtie"
require "active_record/railtie"
require "active_storage/engine"
require "action_controller/railtie"
require "action_mailer/railtie"
require "action_view/railtie"
require "sprockets/railtie"

# Require the gems listed in Gemfile, including any gems
# you've limited to :test, :development, or :production.
Bundler.require(*Rails.groups)

module Deltabadger
  class Application < Rails::Application
    # Initialize configuration defaults for originally generated Rails version.
    config.load_defaults 6.0
    config.autoloader = :classic
    config.autoloader = :zeitwerk
    config.active_storage.replace_on_assign_to_many
    # Settings in config/environments/* take precedence over those specified here.
    # Application configuration can go into files in config/initializers
    # -- all .rb files in that directory are automatically loaded after loading
    # the framework and any gems in your application.

    # Add the Bullet middleware
    if defined?(Bullet)
      config.middleware.use Bullet::Rack
    end

    # explicit app timezone
    config.time_zone = 'UTC'
    config.active_record.default_timezone = :utc

    # Don't generate system test files.
    config.generators.system_tests = nil

    config.i18n.available_locales = [:en, :pl, :es, :de, :nl, :fr, :pt, :ru]
    config.i18n.default_locale = :en
    config.i18n.fallbacks = true

    Raven.configure do |config|
      config.dsn = ENV['SENTRY_DSN']
      config.environments = %w[ production ]
    end

    config.action_view.form_with_generates_remote_forms = false
    # remove Turbo from Asset Pipeline precompilation
    config.after_initialize do
      config.assets.precompile -= Turbo::Engine::PRECOMPILE_ASSETS
    end

    config.api_base = "https://api-iam.intercom.io"
  end
end
