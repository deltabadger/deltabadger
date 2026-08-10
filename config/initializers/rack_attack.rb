# frozen_string_literal: true

# Paths are matched against the REAL routed paths, which carry an optional locale
# prefix (routes.rb mounts Devise inside `scope "(:locale)"` with path_names
# sign_in: 'login') and an optional .format suffix that Rails routes identically.
# Matching a literal string missed both.
module RackAttackPaths
  # Read the CONFIGURED locales, not I18n.available_locales. The i18n railtie applies
  # config.i18n.available_locales after config/initializers have run, so at this point
  # I18n reports whatever the gems on the load path have registered — 75 locales, and not
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
  # Same shape as PASSWORD: an unauthenticated POST that makes the app send mail to an
  # address the caller names. It was the one of the pair without a bound.
  CONFIRMATION = build('/confirmation')
  SETUP       = build('/setup')
  # The OAuth endpoints live outside the locale scope, but still take a format suffix.
  REGISTER    = %r{\A/oauth/register#{FORMAT}\z}
  TOKEN       = %r{\A/oauth/token#{FORMAT}\z}
  AUTHORIZE   = %r{\A/oauth/authorize#{FORMAT}\z}
  # The CSP report endpoint is outside the locale scope for the same reason, and takes a
  # format suffix like everything else. Built here rather than as a literal string so
  # /csp-report.json lands in the same bucket instead of becoming a free bypass.
  CSP_REPORT  = %r{\A/csp-report#{FORMAT}\z}
end

# NOTE ON THE THROTTLE KEY. The key is REMOTE_ADDR — the address the connection was
# accepted from — unless the deployment declares that something in front of it writes the
# forwarded-for header, in which case that header is what tells callers apart and is read
# instead. Which of the two applies is Deltabadger::Application.behind_proxy_from_env.
#
# The choice has to be made here rather than by configuring trusted proxies, because
# ActionDispatch::RemoteIp has no way to express "ignore the header". calculate_ip ends on
#
#   filter_proxies(ips + [remote_addr]).first || ips.last || remote_addr
#
# and the trusted list only decides which hops that first call rejects, never whether the
# header is read. A forwarded address the list does not cover survives the filter and is
# returned by `.first`, so whatever that list holds, a caller supplies a value outside it
# and is keyed on its own input. Widening the list does not help: once it also covers
# REMOTE_ADDR the array empties and `ips.last` hands the forwarded address back regardless.
# RemoteIp's own header comment says as much — without a proxy the remedy is to drop or
# ignore these headers before it runs, not to tune the list. Forwarded-for is also not the
# only header calculate_ip consults, and reading REMOTE_ADDR closes them together rather
# than one at a time. req.ip is not a usable fallback either: it parses the same headers,
# less strictly, and will hand back a value that is not an address at all.
#
# What that buys is a key the request itself cannot restate. What it does not buy is a key
# per person: where a proxy is declared the key is only as good as that proxy, and behind a
# CDN it names the edge that relayed the request rather than whoever sent it. If REMOTE_ADDR
# is somehow absent the key is a shared constant rather than nil, because a throttle block
# returning nil makes rack-attack skip the rule outright, so it has to fail closed.
#
# The bound that does not depend on IP attribution at all is the per-account one: sign-in,
# the second factor and password reset are limited on the user row by :lockable.
def (Rack::Attack).client_ip(req)
  return req.env['REMOTE_ADDR'].presence || 'unattributed' unless Deltabadger::Application.behind_proxy_from_env

  req.env['action_dispatch.remote_ip']&.to_s.presence || req.env['REMOTE_ADDR'].presence || 'unattributed'
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

# Resending a confirmation costs an SMTP round trip on the request thread of a
# single-threaded server, to an address the caller chose.
Rack::Attack.throttle('users/confirmation', limit: 5, period: 60) do |req|
  Rack::Attack.client_ip(req) if req.post? && RackAttackPaths::CONFIRMATION.match?(RackAttackPaths.normalize(req.path))
end

Rack::Attack.throttle('setup', limit: 5, period: 60) do |req|
  Rack::Attack.client_ip(req) if req.post? && RackAttackPaths::SETUP.match?(RackAttackPaths.normalize(req.path))
end

# The CSP report endpoint takes an unauthenticated POST and writes a line to the log, so
# without a bound it is a way to fill the disk and drown everything else in it. What this
# limit does not do is bound the fleet: behind a CDN the key names the edge that relayed
# the request, so hosted containers can share one budget of 30 and lose reports to it.
# Losing reports is the acceptable failure here, and enforcement does not depend on them.
Rack::Attack.throttle('csp-report', limit: 30, period: 60) do |req|
  Rack::Attack.client_ip(req) if req.post? && RackAttackPaths::CSP_REPORT.match?(RackAttackPaths.normalize(req.path))
end
