# frozen_string_literal: true

# Paths are matched against the REAL routed paths, which carry an optional locale
# prefix (routes.rb mounts Devise inside `scope "(:locale)"` with path_names
# sign_in: 'login') and an optional .format suffix that Rails routes identically.
# Matching a literal string missed both.
module RackAttackPaths
  LOCALE = "(?:/(?:#{I18n.available_locales.join('|')}))?"
  # Rails routes any non-slash suffix as :format — .json, .json-api, .v2+json — so the
  # character class must not be narrowed to [A-Za-z0-9] or the suffix becomes a bypass.
  FORMAT = '(?:\.[^/]+)?'

  # Rails routes these to the SAME action, verified with recognize_path against this app:
  #   /login  /login/  //login  /oauth/register/  /oauth//register  /en/login/
  # An anchored regex over the raw req.path matches only the first of each pair, so
  # every throttle below would be bypassed by adding one slash. Normalize first.
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
# handling, which strips RFC1918/loopback hops — enough for kamal-proxy on the same
# host. It does NOT by itself make Cloudflare's edge address resolve to the real
# client: that needs Cloudflare's published ranges in
# `config.action_dispatch.trusted_proxies`, which this app does not set. Until it does,
# hosted traffic arriving through the CDN may share one throttle key. Still strictly
# better than the previous `req.ip`.
def (Rack::Attack).client_ip(req)
  req.env['action_dispatch.remote_ip']&.to_s.presence || req.ip
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
