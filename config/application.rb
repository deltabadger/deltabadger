require_relative 'boot'
require_relative '../lib/middleware/chrome_devtools'

require "rails"
# Pick the frameworks you want:
require "active_model/railtie"
require "active_job/railtie"
require "active_record/railtie"
require "active_storage/engine"
require "action_controller/railtie"
require "action_mailer/railtie"
require "action_view/railtie"
require "action_cable/engine"
require "sprockets/railtie"

# Require the gems listed in Gemfile, including any gems
# you've limited to :test, :development, or :production.
Bundler.require(*Rails.groups)

module Deltabadger
  class Application < Rails::Application
    # Initialize configuration defaults for originally generated Rails version.
    config.load_defaults 7.2
    config.autoloader = :zeitwerk
    # Settings in config/environments/* take precedence over those specified here.
    # Application configuration can go into files in config/initializers
    # -- all .rb files in that directory are automatically loaded after loading
    # the framework and any gems in your application.

    # Add the Bullet middleware
    if defined?(Bullet)
      config.middleware.use Bullet::Rack
    end

    # Silence Chrome DevTools workspace requests
    config.middleware.use Middleware::ChromeDevtools

    # explicit app timezone
    config.time_zone = 'UTC'
    config.active_record.default_timezone = :utc

    #cookie

    config.session_store :cookie_store, key: '_deltabadger_session', expire_after: 30.days

    # Don't generate system test files.
    config.generators.system_tests = nil

    config.i18n.available_locales = [:en, :pl, :es, :de, :nl, :fr, :pt, :ru, :it]
    config.i18n.default_locale = :en
    config.i18n.fallbacks = true

    config.action_view.form_with_generates_remote_forms = false
    # remove Turbo from Asset Pipeline precompilation
    config.after_initialize do
      config.assets.precompile -= Turbo::Engine::PRECOMPILE_ASSETS
    end
  end
end
