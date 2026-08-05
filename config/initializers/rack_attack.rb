# frozen_string_literal: true

# Paths are matched against the REAL routed paths, which carry an optional locale
# prefix (routes.rb mounts Devise inside `scope "(:locale)"` with path_names
# sign_in: 'login') and an optional .format suffix that Rails routes identically.
# Matching a literal string missed both.
module RackAttackPaths
  # Read the CONFIGURED locales, not I18n.available_locales. The i18n railtie applies
  # config.i18n.available_locales after config/initializers have run, so at this point
  # I18n reports whatever the gems on the load path have registered — 74 locales, and not
  # a superset of ours: it omits :el, which routes fine and would have gone unthrottled.
  # routes.rb constrains the :locale segment with the same list, because by the time routes
  # load I18n has caught up.
  # Regexp.union escapes whatever the list contains: a locale carrying a regex
  # metacharacter would otherwise silently rewrite every pattern in this file.
  LOCALE = "(?:/#{Regexp.union(Rails.application.config.i18n.available_locales.map(&:to_s))})?"
  # Rails routes any non-slash suffix as :format — .json, .json-api, .v2+json — so the
  # character class must not be narrowed to [A-Za-z0-9] or the suffix becomes a bypass.
  FORMAT = '(?:\.[^/]+)?'

  # Rails routes these to the SAME action, verified with recognize_path against this app:
  #   /login  /login/  //login  /oauth/register/  /oauth//register  /en/login/
  # An anchored regex matches only the first of each pair, so a raw path would let one
  # extra slash bypass every throttle below. rack-attack already closes that itself —
  # Rack::Attack#call rewrites PATH_INFO through PathNormalizer (which on Rails is
  # ActionDispatch::Journey::Router::Utils.normalize_path) before any throttle block runs,
  # squeezing repeated slashes and stripping the trailing one. This is defense in depth:
  # it keeps the patterns correct against the raw path if that ever stops being true, and
  # it is what the slash tests in test/middleware/rack_attack_test.rb pin.
  def self.normalize(path)
    normalized = path.to_s.squeeze('/')
    normalized.length > 1 ? normalized.chomp('/') : normalized
  end

  def self.build(path)
    %r{\A#{LOCALE}#{path}#{FORMAT}\z}
  end

  LOGIN       = build('/login')
  TWO_FACTOR  = build('/verify_two_factor')
  PASSWORD    = build('/password')
  SETUP       = build('/setup')
  # The OAuth endpoints live outside the locale scope, but still take a format suffix.
  REGISTER    = %r{\A/oauth/register#{FORMAT}\z}
  TOKEN       = %r{\A/oauth/token#{FORMAT}\z}
  AUTHORIZE   = %r{\A/oauth/authorize#{FORMAT}\z}
end

# NOTE ON THE THROTTLE KEY. `action_dispatch.remote_ip` applies Rails' trusted-proxy
# handling, which strips RFC1918/loopback hops — enough for kamal-proxy on the same host.
# It does NOT by itself make Cloudflare's edge address resolve to the real client: that
# needs Cloudflare's published ranges in `config.action_dispatch.trusted_proxies`, which
# this app does not set. Until it does, hosted traffic arriving through the CDN may share
# one throttle key.
def (Rack::Attack).client_ip(req)
  req.env['action_dispatch.remote_ip']&.to_s.presence || req.ip
end

# rack-attack's default response is a bare "Retry later". These rules now match paths this
# app actually routes, so that reaches a human on the sign-in or password-reset page. Give
# them a sentence — the same one whatever tripped, because a response that named the rule
# would tell an attacker which of their guesses was worth repeating.
Rack::Attack.throttled_responder = lambda do |request|
  match_data = request.env['rack.attack.match_data'] || {}
  period = match_data[:period]
  epoch_time = match_data[:epoch_time]

  headers = { 'content-type' => 'text/plain; charset=utf-8' }
  headers['retry-after'] = (period - (epoch_time % period)).to_s if period && epoch_time

  [429, headers, ["#{I18n.t('errors.throttled')}\n"]]
end

Rack::Attack.throttle('oauth/register', limit: 5, period: 60) do |req|
  Rack::Attack.client_ip(req) if req.post? && RackAttackPaths::REGISTER.match?(RackAttackPaths.normalize(req.path))
end

Rack::Attack.throttle('oauth/token', limit: 20, period: 60) do |req|
  Rack::Attack.client_ip(req) if req.post? && RackAttackPaths::TOKEN.match?(RackAttackPaths.normalize(req.path))
end

Rack::Attack.throttle('oauth/authorize', limit: 10, period: 60) do |req|
  Rack::Attack.client_ip(req) if req.get? && RackAttackPaths::AUTHORIZE.match?(RackAttackPaths.normalize(req.path))
end

Rack::Attack.throttle('users/login', limit: 10, period: 60) do |req|
  Rack::Attack.client_ip(req) if req.post? && RackAttackPaths::LOGIN.match?(RackAttackPaths.normalize(req.path))
end

Rack::Attack.throttle('users/verify_two_factor', limit: 5, period: 60) do |req|
  Rack::Attack.client_ip(req) if req.post? && RackAttackPaths::TWO_FACTOR.match?(RackAttackPaths.normalize(req.path))
end

# POST requests a reset email; PATCH/PUT submits the new password and the OTP. The
# form is method: :patch, so a POST-only rule would leave the guessable half open.
Rack::Attack.throttle('users/password', limit: 5, period: 60) do |req|
  if %w[POST PATCH PUT].include?(req.request_method) && RackAttackPaths::PASSWORD.match?(RackAttackPaths.normalize(req.path))
    Rack::Attack.client_ip(req)
  end
end

Rack::Attack.throttle('setup', limit: 5, period: 60) do |req|
  Rack::Attack.client_ip(req) if req.post? && RackAttackPaths::SETUP.match?(RackAttackPaths.normalize(req.path))
end
