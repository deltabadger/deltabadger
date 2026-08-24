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

    # Rails advertises every stylesheet_link_tag and javascript_include_tag asset in a
    # Link: rel=preload response header. Turbo then fetches pages the browser already holds
    # those assets for, so each visit starts preloads nothing consumes and the console fills
    # with "preloaded but not used". The tags themselves already fetch what the page needs;
    # the header only duplicates them, and it never covered application.js anyway — Rails
    # omits type: "module" scripts from it.
    config.action_view.preload_links_header = false

    # explicit app timezone
    config.time_zone = 'UTC'
    config.active_record.default_timezone = :utc

    # ActionDispatch::RemoteIp raises IpSpoofAttackError when a request carries Client-IP and
    # a disagreeing X-Forwarded-For. That raise happens inside Rails::Rack::Logger, which runs
    # ABOVE the middleware that turns exceptions into responses and above Rack::Attack — so
    # two headers on any path, from anyone, produce a 500 that no throttle can bound and no
    # rescue_from can catch.
    #
    # Turning the check off costs nothing here, because nothing downstream trusts what it
    # inspects. Rack::Attack.client_ip reads REMOTE_ADDR outright unless the deployment
    # declares a proxy, and where one is declared the address comes from that proxy. The check
    # does not decide either of those; all it does is decide whether to raise. Compare
    # disabling it with the alternative of listing trusted proxies: that list changes which
    # hops are filtered, never whether the headers are read, so it would not stop this raise.
    config.action_dispatch.ip_spoofing_check = false

    # ENV carries a setting as free text, so a resolver has to rule on what a spelling meant.
    # The spellings are listed out rather than run through ActiveModel::Type::Boolean
    # because that cast has no verdict for an unrecognised value: its false list is closed,
    # so 'no' — and every typo — comes back true. Neither setting below may have an
    # unrecognised value mean on, so nothing is guessed here: anything unrecognised returns
    # nil and the caller falls through to what it can infer about the deployment. One list,
    # so the two resolvers cannot drift into disagreeing about what an operator wrote.
    def self.env_boolean(value)
      case value.to_s.strip.downcase
      when '1', 't', 'true', 'y', 'yes', 'on' then true
      when '0', 'f', 'false', 'n', 'no', 'off' then false
      end
    end

    # Deployments that terminate TLS in front of the app do not necessarily pass FORCE_SSL,
    # so the configured root URL is the signal: if the app is reached over https then https
    # is what it must insist on. A plain-http self-hosted or LAN install keeps its plain
    # http, and FORCE_SSL overrides the inference in either direction.
    #
    # Turning SSL on for a plain-http install points every redirect and every generated URL
    # at a host with no TLS listener, which is a site the browser cannot reach at all — so
    # anything unrecognised falls through to the root URL, which is the branch that is
    # secure by default without guessing.
    def self.force_ssl_from_env(env = ENV)
      explicit = env_boolean(env['FORCE_SSL'])
      return explicit unless explicit.nil?

      env['APP_ROOT_URL'].to_s.start_with?('https://')
    end

    # Whether something in front of this deployment writes the forwarded-for header, and so
    # whether that header describes the caller or is only what the caller typed. The
    # rack-attack throttle key turns on the answer.
    #
    # That is the same fact about the deployment force_ssl reads, so it reads the same
    # signals: nothing here binds Puma to TLS, so an https root URL — or an explicit
    # FORCE_SSL — means TLS terminates in front, and whatever terminates it is what writes
    # the header. README's deployment advice is to put nginx or Traefik there for exactly
    # that reason. BEHIND_PROXY is for the installs where the two facts come apart: true for
    # a proxy that terminates plain http, false for one that serves TLS from Ruby itself.
    def self.behind_proxy_from_env(env = ENV)
      explicit = env_boolean(env['BEHIND_PROXY'])
      return explicit unless explicit.nil?

      force_ssl_from_env(env)
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
