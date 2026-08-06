require_relative 'boot'
require_relative '../lib/middleware/chrome_devtools'

require "rails"
# Pick the frameworks you want:
require "active_model/railtie"
require "active_job/railtie"
require "active_record/railtie"
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
    config.load_defaults 8.1
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

    # Deployments that terminate TLS in front of the app do not necessarily pass FORCE_SSL,
    # so the configured root URL is the signal: if the app is reached over https then https
    # is what it must insist on. A plain-http self-hosted or LAN install keeps its plain
    # http, and FORCE_SSL overrides the inference in either direction.
    #
    # The spellings are listed out rather than run through ActiveModel::Type::Boolean
    # because that cast has no verdict for an unrecognised value: its false list is closed,
    # so 'no' — and every typo — comes back true. On this setting an unrecognised value
    # must never be able to mean on. Turning SSL on for a plain-http install points every
    # redirect and every generated URL at a host with no TLS listener, which is a site the
    # browser cannot reach at all. Anything unrecognised falls through to the root URL,
    # which is the branch that is secure by default without guessing.
    def self.force_ssl_from_env(env = ENV)
      explicit = env['FORCE_SSL'].to_s.strip.downcase
      return true if %w[1 t true y yes on].include?(explicit)
      return false if %w[0 f false n no off].include?(explicit)

      env['APP_ROOT_URL'].to_s.start_with?('https://')
    end

    #cookie

    # secure and same_site both restate what the stack derives on its own today — the
    # middleware sets secure whenever force_ssl is on, and SameSite=Lax is the default.
    # Stating them at the declaration keeps the cookie's intent readable and pins it if
    # either default moves. secure comes from the same signal as force_ssl so that a
    # plain-http install is not handed a cookie its browser will refuse to send back.
    config.session_store :cookie_store, key: '_deltabadger_session', expire_after: 30.days,
                                        secure: force_ssl_from_env, same_site: :lax

    config.action_mcp.name = "Deltabadger"

    # Return 401 with WWW-Authenticate header for unauthenticated MCP requests.
    # Required by RFC 9728 so OAuth clients can discover the authorization flow.
    require_relative '../lib/middleware/mcp_oauth_challenge'
    config.middleware.use McpOauthChallenge

    # Don't generate system test files.
    config.generators.system_tests = nil

    config.i18n.available_locales = %i[en pl es de nl fr pt ru it bg el sv da cs sk]
    config.i18n.default_locale = :en
    config.i18n.fallbacks = true

    # remove Turbo from Asset Pipeline precompilation
    config.after_initialize do
      config.assets.precompile -= Turbo::Engine::PRECOMPILE_ASSETS
    end
  end
end
