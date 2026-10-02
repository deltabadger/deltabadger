# The Rails half of the OAuth parity harness (rust/tests/oauth.rs).
#   bin/rails runner script/rust/oauth.rb grid <root>       # one install per scenario and mode, with scenario.json; and catalogue.json
#   bin/rails runner script/rust/oauth.rb play <root> <leg> # Rails plays its legs number <leg>: transcript.json, state.json
# Run as script/rust/pages.rb is: RAILS_ENV=test, scratch *_DATABASE_URLs with the schemas loaded.
# OAUTH=token,refresh limits the grid to scenarios whose name starts with one of the prefixes.
#
# A scenario is a list of steps. A step with 'cut' starts a new leg: new browsers and new rate-limit
# counters, as after a restart. A scenario with cuts is also run with the legs taken in turns by Rails
# and by Rust on one database (modes rails_first and rust_first): the switch and the handback.
OAUTH_PARITY = true
load Rails.root.join('script/rust/pages.rb')
require 'nokogiri'

module OauthParity
  extend ActiveSupport::Testing::TimeHelpers

  module_function

  VERIFIER = 'dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk'.freeze
  CHALLENGE = Base64.urlsafe_encode64(Digest::SHA256.digest(VERIFIER), padding: false).freeze
  REDIRECT = 'https://client.example/callback'.freeze
  MODES = %w[rails rust rails_first rust_first].freeze
  SEEDED_AT = '2026-01-01 00:00:00.000000'.freeze
  PERSONAL_TOKEN = ('ab' * 32).freeze

  # 'client' names the browser a step is sent from. Pages come from 'main'; the calls an OAuth client
  # makes itself (register, token, revoke) come from 'api', which never holds a cookie.
  def get(path, query = nil, headers = {}) = { 'do' => 'get', 'path' => path, 'query' => query, 'headers' => headers }.compact
  def post_form(path, form, headers = {}) = { 'do' => 'post', 'path' => path, 'form' => form, 'headers' => headers, 'client' => 'api' }
  def post_json(path, json, headers = {}) = { 'do' => 'post', 'path' => path, 'json' => json, 'headers' => headers, 'client' => 'api' }
  def submit(action, method, set = {}) = { 'do' => 'submit', 'action' => action, 'method' => method, 'set' => set }
  # In-process steps. 'resolve' is the bearer-token check as a read (OauthBearerTokenResolver, web::bearer::resolve).
  # 'use' is what an endpoint does with the header: for Rails the same call, for Rust `authenticate`, which also
  # retires the refresh token the access token came from. 'uri' asks the redirect-URI rule directly.
  def resolve(authorization, scope = 'mcp') = { 'do' => 'resolve', 'authorization' => authorization, 'scope' => scope }
  def use(authorization = 'Bearer $access_token') = { 'do' => 'use', 'authorization' => authorization, 'scope' => 'mcp' }
  def uri(url, registered) = { 'do' => 'uri', 'url' => url, 'registered' => registered }

  def register(json = {}) = post_json('/oauth/register', { 'client_name' => 'Claude', 'redirect_uris' => [REDIRECT] }.merge(json).compact)

  def sign_in(email: 'owner@example.com', password: Pages::PASSWORD)
    [get('/login'), submit('/login', 'post', 'user[email]' => email, 'user[password]' => password)]
  end

  def authorize_query(over = {})
    { 'client_id' => '$client_id', 'redirect_uri' => REDIRECT, 'response_type' => 'code', 'scope' => 'mcp', 'state' => 'st4te',
      'code_challenge' => CHALLENGE, 'code_challenge_method' => 'S256' }.merge(over).compact
  end

  def authorize(over = {}) = get('/oauth/authorize', authorize_query(over))
  def approve(set = {}) = submit('/oauth/authorize', 'post', set)
  def deny(set = {}) = submit('/oauth/authorize', 'delete', set)

  def exchange(over = {}, headers = {})
    post_form('/oauth/token', { 'grant_type' => 'authorization_code', 'code' => '$code', 'redirect_uri' => REDIRECT,
                                'code_verifier' => VERIFIER, 'client_id' => '$client_id' }.merge(over).compact, headers)
  end

  def refresh(token = '$refresh_token', over = {})
    post_form('/oauth/token', { 'grant_type' => 'refresh_token', 'refresh_token' => token, 'client_id' => '$client_id' }.merge(over).compact)
  end

  def revoke(token, over = {}) = post_form('/oauth/revoke', { 'token' => token, 'client_id' => '$client_id' }.merge(over).compact)

  # A client registered and authorised by the owner, with its code not yet exchanged.
  def authorized(registration = {}, query = {}) = [register(registration)] + sign_in + [authorize(query), approve]
  # The same, holding its first tokens.
  def connected(registration = {}, query = {}) = authorized(registration, query) + [exchange]

  def cut(step) = step.merge('cut' => true)
  def later(seconds, step) = step.merge('advance' => seconds)
  def expect(status, step) = step.merge('expect' => status)
  def with_sql(step, *sql) = step.merge('sql' => sql)
  def from(client, steps) = Array.wrap(steps).map { |step| step.merge('client' => client) }

  # The per-user REST token and its application, as User#mint_personal_token! leaves them (fixed values, so
  # every install of the grid holds the same rows).
  PERSONAL_SEED = [
    "INSERT INTO oauth_applications (name, uid, redirect_uri, scopes, confidential, personal_access_token, personal_owner_id, created_at, updated_at) " \
    "VALUES ('Personal API token', 'personal-application-uid', 'https://localhost/personal-access-token', 'api', 0, 1, 1, '#{SEEDED_AT}', '#{SEEDED_AT}')",
    "INSERT INTO oauth_access_tokens (application_id, resource_owner_id, token, scopes, created_at) " \
    "SELECT id, 1, '#{PERSONAL_TOKEN}', 'api', '#{SEEDED_AT}' FROM oauth_applications WHERE uid = 'personal-application-uid'"
  ].freeze

  SPACED_BASIC = "Basic #{Base64.strict_encode64('confidential-uid:s3cret').insert(6, ' ')}".freeze
  UNPADDED_BASIC = "Basic #{Base64.strict_encode64('confidential-uid:s3cret').delete('=')}".freeze
  CONFIDENTIAL_SEED = "INSERT INTO oauth_applications (name, uid, secret, redirect_uri, scopes, confidential, created_at, updated_at) " \
                      "VALUES ('Older client', 'confidential-uid', 's3cret', '#{REDIRECT}', 'mcp', 1, '#{SEEDED_AT}', '#{SEEDED_AT}')".freeze

  # A code as Rails issued it for a `plain` challenge: the challenge is the verifier.
  def grant_row(code)
    "INSERT INTO oauth_access_grants (application_id, resource_owner_id, token, expires_in, redirect_uri, scopes, code_challenge, code_challenge_method, created_at) " \
      "VALUES (1, 1, '#{code}', 600, '#{REDIRECT}', 'mcp', '#{VERIFIER}', 'plain', '2026-09-10 12:00:30.123456')"
  end

  def token_row(token, over = {})
    row = { 'application_id' => 1, 'resource_owner_id' => 1, 'token' => token, 'scopes' => 'mcp', 'expires_in' => 3600,
            'created_at' => '2026-09-10 12:00:30.123456' }.merge(over)
    "INSERT INTO oauth_access_tokens (#{row.keys.join(', ')}) VALUES (#{row.values.map { |v| v.nil? ? 'NULL' : ActiveRecord::Base.connection.quote(v) }.join(', ')})"
  end

  def control_enabled = Pages.user('mcp_settings' => { 'tool_permissions' => { 'start_bot' => true, 'stop_bot' => true, 'market_buy' => true,
                                                                               'list_bots' => false } },
                                   'rest_settings' => { 'tool_permissions' => { 'list_bots' => true, 'list_exchanges' => true } })

  def scenarios
    {}.merge(hosts, discovery, registration, authorization, consent, token, refresh_scenarios, revocation, resolver, uri_rules, crossing)
  end

  # ALLOWED_HOSTS: 'allowed_hosts' on a scenario is its value. Rails then answers as production does, with
  # ActionDispatch::HostAuthorization in front of everything (`guarded`).
  def hosts
    allow = { 'allowed_hosts' => 'app.example, .apps.example' }
    document = '/.well-known/oauth-authorization-server'
    at = ->(host, more = {}) { { 'Host' => host }.merge(more) }
    nothing = { 'grant_type' => 'refresh_token', 'refresh_token' => 'nothing', 'client_id' => 'nobody' }
    {
      'host_allowed_and_refused' => allow.merge('steps' => [
        expect(200, get('/up')), expect(200, get('/up', nil, at.('APP.example:8443'))), expect(403, get('/up', nil, at.('evil.example'))),
        expect(200, get('/up', nil, at.('bot.apps.example'))), expect(403, get('/up', nil, at.('a.b.apps.example'))), expect(200, get('/up', nil, at.('127.0.0.1:3000'))),
        expect(403, get('/login', nil, at.('evil.example'))), expect(403, get('/login', nil, at.('evil.example', 'X-Requested-With' => 'XMLHttpRequest'))),
        expect(403, get('/logo_email_app.png', nil, at.('evil.example'))), expect(403, get(document, nil, at.('evil.example'))),
        expect(403, post_form('/oauth/token', nothing, at.('evil.example'))), expect(400, post_form('/oauth/token', nothing, at.('app.example')))
      ]),
      # A proxy's X-Forwarded-Host is what the documents are built from, so it is checked too: its last entry.
      'host_forwarded_by_a_proxy' => allow.merge('steps' => [
        expect(403, get(document, nil, at.('app.example', 'X-Forwarded-Host' => 'evil.example'))),
        expect(200, get(document, nil, at.('app.example', 'X-Forwarded-Host' => 'bot.apps.example', 'X-Forwarded-Proto' => 'https'))),
        expect(200, get(document, nil, at.('app.example', 'X-Forwarded-Host' => 'evil.example, app.example'))),
        expect(403, get(document, nil, at.('app.example', 'X-Forwarded-Host' => 'app.example, evil.example'))),
        expect(403, get(document, nil, at.('evil.example', 'X-Forwarded-Host' => 'app.example'))),
        expect(200, get('/.well-known/oauth-protected-resource', nil, at.('app.example', 'X-Forwarded-Host' => ' ')))
      ]),
      'host_refused_requests_write_and_count_nothing' => allow.merge('steps' =>
        Array.new(6) { expect(403, register.merge('headers' => at.('evil.example'))) } + Array.new(5) { expect(201, register) } + [expect(429, register)]),
      # Without ALLOWED_HOSTS production clears config.hosts: every host is served, and named in the documents.
      'host_any_without_a_list' => { 'steps' => [expect(200, get(document, nil, at.('whatever.example'))),
                                                 expect(200, get('/up', nil, at.('evil.example', 'X-Forwarded-Host' => 'other.example')))] }
    }
  end

  def discovery
    {
      'discovery_documents' => { 'steps' => [get('/.well-known/oauth-authorization-server'), get('/.well-known/oauth-protected-resource'),
                                             { 'do' => 'head', 'path' => '/.well-known/oauth-authorization-server', 'headers' => {} }] },
      'discovery_behind_a_proxy' => { 'steps' => [
        get('/.well-known/oauth-authorization-server', nil, 'X-Forwarded-Host' => 'bot.example', 'X-Forwarded-Proto' => 'https'),
        get('/.well-known/oauth-protected-resource', nil, 'X-Forwarded-Host' => 'bot.example', 'X-Forwarded-Proto' => 'https'),
        get('/.well-known/oauth-protected-resource', nil, 'Host' => 'other.example:8443')
      ] },
      'discovery_signed_in' => { 'steps' => sign_in + [get('/.well-known/oauth-authorization-server')] },
      'absent_well_known' => { 'steps' => [get('/.well-known/oauth-protected-resource/mcp'), get('/.well-known/openid-configuration'),
                                           get('/.well-known/oauth-authorization-server/mcp')] },
      'unrouted_locale_before_well_known' => { 'steps' => [get('/de/.well-known/oauth-authorization-server')] }
    }
  end

  def registration
    long = "https://client.example/#{'a' * 1977}" # 2,000 characters
    {
      'register_minimal' => { 'steps' => [expect(201, post_json('/oauth/register', { 'redirect_uris' => [REDIRECT] }))] },
      'register_full' => { 'steps' => [expect(201, register('client_name' => "  #{'N' * 120}  ", 'scope' => 'mcp api mcp',
                                                                   'redirect_uris' => [REDIRECT, 'http://127.0.0.1:8123/cb?x=1', long]))] },
      'register_form_encoded' => { 'steps' => [post_form('/oauth/register', { 'client_name' => 'Form', 'redirect_uris[]' => [REDIRECT, 'http://localhost/cb'],
                                                                              'scope' => 'api' }),
                                               post_form('/oauth/register', { 'redirect_uris' => REDIRECT })] },
      'register_blank_name_and_scope' => { 'steps' => [register('client_name' => '   ', 'scope' => '  ')] },
      'register_json_charset' => { 'steps' => [register({}).merge('headers' => { 'Content-Type' => 'application/json; charset=utf-8' })] },
      'register_no_redirect_uris' => { 'steps' => [post_json('/oauth/register', { 'client_name' => 'x' }), post_json('/oauth/register', { 'redirect_uris' => [] }),
                                                    post_json('/oauth/register', { 'redirect_uris' => '' }),
                                                    post_form('/oauth/register', {}).merge('headers' => { 'Content-Type' => 'text/plain' })] },
      'register_too_many_uris' => { 'steps' => [register('redirect_uris' => Array.new(6) { |i| "https://client.example/#{i}" })] },
      'register_uri_too_long' => { 'steps' => [register('redirect_uris' => ["#{long}a"])] },
      'register_uri_not_http' => { 'steps' => [register('redirect_uris' => ['myapp://callback']), register('redirect_uris' => ['/relative']),
                                                register('redirect_uris' => ['https:///nohost']), register('redirect_uris' => ['https://exa mple.com/cb']),
                                                register('redirect_uris' => [REDIRECT, 123])] },
      'register_uri_fragment' => { 'steps' => [register('redirect_uris' => ['https://client.example/cb#frag']),
                                                register('redirect_uris' => ['https://client.example/cb#', REDIRECT, 'http://localhost/cb#x'])] },
      'register_scope_refused' => { 'steps' => [register('scope' => 'mcp admin'), register('scope' => 'MCP')] },
      'register_malformed_json' => { 'steps' => [post_json('/oauth/register', '{"redirect_uris": [')] },
      'register_from_a_signed_in_browser' => { 'steps' => sign_in + from('main', [register({}).merge('headers' => { 'Origin' => 'http://evil.example' })]) +
        [expect(302, get('/login'))] },
      # Rails' `params` is the query and the body, and the query wins.
      'register_parameters_in_the_query' => { 'steps' => [
        expect(201, post_json('/oauth/register?redirect_uris%5B%5D=https%3A%2F%2Fq.example%2Fcb&redirect_uris%5B%5D=https%3A%2F%2Fq.example%2Ftwo&scope=api&client_name=Query', {})),
        expect(201, post_form('/oauth/register?redirect_uris=https%3A%2F%2Fq.example%2Fone', {})),
        expect(400, post_form('/oauth/register?scope=mcp', {}))
      ] },
      'register_query_wins_over_the_body' => { 'steps' => [
        expect(201, register('scope' => 'mcp', 'client_name' => 'Body')
          .merge('path' => '/oauth/register?scope=api&client_name=Query&redirect_uris%5B%5D=https%3A%2F%2Fq.example%2Fcb')),
        expect(400, register.merge('path' => '/oauth/register?scope=admin')),
        expect(400, register.merge('path' => '/oauth/register?redirect_uris%5B%5D=myapp%3A%2F%2Fcb')),
        expect(201, post_form('/oauth/register?client_name=Query', { 'client_name' => 'Form', 'redirect_uris[]' => [REDIRECT] }))
      ] },
      # Doorkeeper validates the stored text split at whitespace: a space in a query makes two entries of one URI.
      'authorize_registered_uri_with_whitespace' => { 'steps' => [
        expect(400, register('redirect_uris' => ['https://client.example/cb?x=hello world'])), expect(400, register('redirect_uris' => ["https://client.example/cb?x=a\tb"])),
        expect(400, register('redirect_uris' => ['https://client.example/cb?x=1 javascript:alert(1)'])), expect(400, register('redirect_uris' => ['https://client.example/cb?x=1 %zz'])),
        expect(201, register('redirect_uris' => ['https://client.example/cb?x=1 https://b.example/cb']))
      ] + sign_in + [expect(200, authorize('redirect_uri' => 'https://b.example/cb')),
                     expect(400, authorize('redirect_uri' => 'https://client.example/cb?x=1 https://b.example/cb'))] },
      'throttle_register' => { 'steps' => Array.new(5) { register } + [expect(429, post_json('/oauth//register/', { 'redirect_uris' => [REDIRECT] })),
                                                                       later(30, register)] },
      'unrouted_get_register' => { 'steps' => [get('/oauth/register')] }
    }
  end

  def authorization
    {
      'authorize_signed_out' => { 'steps' => [register, expect(302, authorize), get('/login'),
                                              expect(303, submit('/login', 'post', 'user[email]' => 'owner@example.com', 'user[password]' => Pages::PASSWORD)),
                                              expect(200, authorize)] },
      'authorize_signed_out_two_factor' => { 'user' => Pages.two_factor_user, 'steps' => [
        register, authorize, get('/login'), submit('/login', 'post', 'user[email]' => 'owner@example.com', 'user[password]' => Pages::PASSWORD),
        get('/verify_two_factor'), submit('/verify_two_factor', 'post', 'user[otp_code_token]' => Pages.code_at(Pages::AT)), expect(200, authorize)
      ] },
      'authorize_signed_out_unknown_client' => { 'steps' => [expect(302, authorize('client_id' => 'nobody'))] + sign_in },
      'authorize_errors_client' => { 'sql' => PERSONAL_SEED, 'steps' => [register('redirect_uris' => [REDIRECT, 'http://127.0.0.1/cb'])] + sign_in + [
        authorize('client_id' => nil), authorize('client_id' => 'nobody'), authorize('redirect_uri' => nil),
        authorize('redirect_uri' => 'https://client.example/other'), authorize('redirect_uri' => 'https://client.example/callback/'),
        authorize('redirect_uri' => 'http://127.0.0.1:9000/other'), authorize('redirect_uri' => 'http://user@127.0.0.1:9000/cb'),
        authorize('client_id' => 'personal-application-uid', 'redirect_uri' => 'https://localhost/personal-access-token', 'scope' => 'api')
      ] },
      'authorize_errors_request' => { 'steps' => [register] + sign_in + [
        authorize('response_type' => nil), authorize('response_type' => 'token'), authorize('response_mode' => 'form_post'),
        authorize('scope' => 'mcp admin'), authorize('scope' => 'api'), authorize('scope' => "mcp\tapi"),
        authorize('code_challenge' => nil), authorize('code_challenge_method' => nil), authorize('code_challenge_method' => 's256')
      ] },
      'authorize_loopback_port' => { 'steps' => [register('redirect_uris' => ['http://127.0.0.1/cb', 'http://localhost:3334/cb'])] + sign_in + [
        authorize('redirect_uri' => 'http://127.0.0.1:53211/cb'), approve, authorize('redirect_uri' => 'http://localhost:9/cb'),
        authorize('redirect_uri' => 'http://[::1]:9/cb')
      ] },
      # Ruby rewrites a query as it parses it (a space is %20, a tab is nothing), and loopback URIs are compared parsed. A code
      # asked for with the space is issued and can never be redeemed: its stored redirect URI is two entries to the exchange.
      'token_loopback_query_is_rewritten' => { 'steps' => [register('redirect_uris' => ['http://127.0.0.1/cb?x=hello%20world'])] + sign_in + [
        expect(200, authorize('redirect_uri' => 'http://127.0.0.1:53211/cb?x=hello world')), expect(302, approve),
        expect(400, exchange('redirect_uri' => 'http://127.0.0.1:53211/cb?x=hello world')),
        expect(200, authorize('redirect_uri' => 'http://127.0.0.1:53211/cb?x=hello%20world')), expect(302, approve),
        expect(200, exchange('redirect_uri' => 'http://127.0.0.1:7/cb?x=hello%20world')), expect(400, authorize('redirect_uri' => 'http://127.0.0.1:53211/cb?x=hello+world'))
      ] },
      'authorize_signed_out_german_account' => { 'user' => Pages.user('locale' => 'de'), 'steps' => [
        register, authorize, get('/login'), submit('/login', 'post', 'user[email]' => 'owner@example.com', 'user[password]' => Pages::PASSWORD),
        expect(200, authorize('locale' => 'de')), approve
      ] },
      'tighter_plain_challenge' => { 'steps' => [register] + sign_in + [authorize('code_challenge' => VERIFIER, 'code_challenge_method' => 'plain')] },
      'authorize_head' => { 'steps' => [register] + sign_in + [{ 'do' => 'head', 'path' => '/oauth/authorize', 'query' => authorize_query, 'headers' => {} }] },
      'throttle_authorize' => { 'steps' => [register] + sign_in + Array.new(10) { authorize } + [expect(429, authorize), later(30, authorize)] },
      'not_ported_native_code_page' => { 'steps' => sign_in + [get('/oauth/authorize/native', 'code' => 'abc')] },
      'unrouted_locale_before_authorize' => { 'steps' => [register] + sign_in + [get('/de/oauth/authorize', authorize_query)] }
    }
  end

  def consent
    both = { 'scope' => 'mcp api' }
    {
      'consent_first' => { 'steps' => [register] + sign_in + [expect(200, authorize), expect(302, approve)] },
      'consent_both_scopes' => { 'user' => control_enabled, 'steps' => [register(both)] + sign_in + [
        authorize(both), approve('granted_mcp_groups[]' => %w[read control trade tax], 'granted_rest_groups[]' => %w[read]),
        authorize(both), approve('granted_mcp_groups[]' => %w[read]), authorize(both),
        authorize('scope' => 'mcp'), approve('granted_mcp_groups[]' => nil), authorize(both), approve
      ] },
      'consent_api_only' => { 'steps' => [register('scope' => 'api')] + sign_in + [authorize('scope' => 'api'), approve, authorize('scope' => nil)] },
      'consent_default_scope' => { 'steps' => [register(both)] + sign_in + [authorize('scope' => nil), approve] },
      'consent_client_name_is_text' => { 'steps' => [register('client_name' => '<script>alert(1)</script> & "Q"')] + sign_in + [authorize] },
      'consent_without_state' => { 'steps' => [register('redirect_uris' => ['https://client.example/callback?keep=1&state=theirs'])] + sign_in + [
        authorize('state' => nil, 'redirect_uri' => 'https://client.example/callback?keep=1&state=theirs'), approve
      ] },
      'consent_state_is_text' => { 'steps' => [register] + sign_in + [authorize('state' => %(a b&c="d"<e>+é)), approve] },
      'consent_fragment_mode' => { 'steps' => [register] + sign_in + [authorize('response_mode' => 'fragment'), approve, authorize('response_mode' => 'fragment'),
                                                                     deny] },
      'consent_groups_not_in_the_form' => { 'steps' => [register] + sign_in + [
        authorize, approve('granted_mcp_groups[]' => %w[bogus tax tax], 'granted_rest_groups[]' => %w[read]),
        authorize, approve('granted_mcp_groups[]' => nil, 'granted_mcp_groups' => 'read')
      ] },
      # The redirect is the URI as Ruby writes it back, not as it was registered: scheme in lower case, no default port.
      'token_redirect_is_written_as_ruby_writes_a_uri' => { 'steps' => [register('redirect_uris' => ['HTTPS://Client.example:0443/Callback'])] + sign_in + [
        authorize('redirect_uri' => 'HTTPS://Client.example:0443/Callback'), expect(302, approve),
        expect(200, exchange('redirect_uri' => 'HTTPS://Client.example:0443/Callback')), authorize('redirect_uri' => 'HTTPS://Client.example:0443/Callback'), expect(302, deny)
      ] },
      # The ticked groups are read from Rails' `params` too: the query's, when the query names them.
      'consent_groups_in_the_query' => { 'user' => control_enabled, 'steps' => [register] + sign_in + [
        authorize, expect(302, approve('granted_mcp_groups[]' => nil).merge('query' => { 'granted_mcp_groups[]' => %w[trade] })),
        authorize, expect(302, approve('granted_mcp_groups[]' => %w[read control]).merge('query' => { 'granted_mcp_groups[]' => %w[tax] })),
        authorize, expect(302, approve.merge('query' => { 'granted_mcp_groups' => 'read' }))
      ] },
      'consent_denied' => { 'steps' => [register] + sign_in + [authorize, expect(302, deny)] },
      'consent_tampered_approve' => { 'steps' => [register] + sign_in + [
        authorize, approve('redirect_uri' => 'https://evil.example/cb'), authorize, approve('client_id' => 'nobody'),
        authorize, approve('scope' => 'mcp admin'), authorize, approve('code_challenge' => nil), authorize, approve('response_type' => 'token'),
        authorize, approve('client_id' => nil)
      ] },
      'consent_tampered_deny' => { 'steps' => [register] + sign_in + [
        authorize, deny('redirect_uri' => 'https://evil.example/cb'), authorize, deny('client_id' => 'nobody'), authorize, deny('scope' => 'mcp admin'),
        authorize, deny('response_type' => 'token')
      ] },
      'stricter_approve_without_token' => { 'steps' => [register] + sign_in + [authorize, approve('authenticity_token' => 'forged')] },
      'stricter_approve_foreign_origin' => { 'steps' => [register] + sign_in + [authorize, approve.merge('headers' => { 'Origin' => 'http://evil.example' })] },
      'stricter_deny_without_token' => { 'steps' => [register] + sign_in + [authorize, deny('authenticity_token' => nil)] },
      'consent_signed_out_with_a_token' => { 'steps' => [register] + sign_in + [authorize] + from('other', [
        get('/login'), post_form('/oauth/authorize', authorize_query).merge('csrf' => 'header'),
        post_form('/oauth/authorize', authorize_query.merge('_method' => 'delete')).merge('csrf' => 'header'), get('/login')
      ]) },
      'consent_fragment_by_hand' => { 'steps' => [register] + sign_in + [authorize, approve('response_mode' => 'fragment'), authorize,
                                                                         deny('response_mode' => 'fragment'), authorize, deny('response_type' => nil)] },
      'stricter_approve_signed_out' => { 'steps' => [register] + sign_in + [authorize] + from('other', [approve.merge('page_of' => 'main')]) }
    }
  end

  def token
    {
      'token_exchange' => { 'steps' => authorized + [expect(200, exchange)] },
      'token_exchange_both_scopes' => { 'steps' => authorized({ 'scope' => 'mcp api' }, { 'scope' => 'api mcp' }) + [exchange] },
      'token_exchange_json_body' => { 'steps' => authorized + [post_json('/oauth/token', { 'grant_type' => 'authorization_code', 'code' => '$code',
                                                                                         'redirect_uri' => REDIRECT, 'code_verifier' => VERIFIER,
                                                                                         'client_id' => '$client_id' })] },
      'token_exchange_from_a_browser' => { 'steps' => authorized + from('main', [exchange({}, 'Origin' => 'http://evil.example')]) + [expect(302, get('/login'))] },
      'token_exchange_with_bearer_header' => { 'steps' => authorized + [exchange({}, 'Authorization' => 'Bearer whatever')] },
      'token_code_reused' => { 'steps' => authorized + [exchange, expect(400, exchange), resolve('Bearer $access_token')] },
      'token_code_last_second' => { 'steps' => authorized + [later(600, expect(200, exchange))] },
      'token_code_expired' => { 'steps' => authorized + [later(601, expect(400, exchange))] },
      'token_wrong_verifier' => { 'steps' => authorized + [expect(400, exchange('code_verifier' => "#{VERIFIER}x")), expect(200, exchange)] },
      'token_verifier_is_the_challenge' => { 'steps' => authorized + [exchange('code_verifier' => CHALLENGE)] },
      'token_wrong_redirect_uri' => { 'steps' => authorized + [exchange('redirect_uri' => 'https://client.example/other')] },
      'token_loopback_other_port' => { 'steps' => [register('redirect_uris' => ['http://127.0.0.1/cb'])] + sign_in + [
        authorize('redirect_uri' => 'http://127.0.0.1:5000/cb'), approve, exchange('redirect_uri' => 'http://127.0.0.1:6000/cb')
      ] },
      'token_wrong_client' => { 'steps' => authorized + [register, exchange('client_id' => '$client_id.2'), exchange('client_id' => '$client_id.1')] },
      'token_unknown_client' => { 'steps' => authorized + [expect(401, exchange('client_id' => 'nobody'))] },
      'token_unknown_code' => { 'steps' => authorized + [exchange('code' => 'nothing')] },
      'token_missing_parameters' => { 'steps' => authorized + [
        exchange('grant_type' => nil), exchange('code' => nil), exchange('code_verifier' => nil), exchange('redirect_uri' => nil),
        exchange('client_id' => nil), exchange('code' => '', 'client_id' => ''), post_form('/oauth/token', {})
      ] },
      'token_unsupported_grant_types' => { 'steps' => authorized + [
        exchange('grant_type' => 'password'), exchange('grant_type' => 'client_credentials'), exchange('grant_type' => 'implicit'),
        exchange('grant_type' => 'AUTHORIZATION_CODE')
      ] },
      # Rails' `params` holds the query too, and there the query wins; the client's own credentials are read from the body alone.
      'token_parameters_in_the_query' => { 'steps' => authorized + [
        exchange('client_id' => nil).merge('path' => '/oauth/token?client_id=$client_id'),
        exchange('code' => 'nothing').merge('path' => '/oauth/token?code=$code&grant_type=authorization_code'),
        revoke('nothing').merge('path' => '/oauth/revoke?token=$access_token'), resolve('Bearer $access_token')
      ] },
      'token_client_secret_sent' => { 'steps' => authorized + [exchange('client_secret' => 'anything'), exchange] },
      'token_basic_header' => { 'steps' => authorized + [exchange({}, 'Authorization' => 'Basic bm9ib2R5OnNlY3JldA=='), exchange] },
      'token_public_client_other_ways' => { 'steps' => authorized + [
        exchange({}, 'Authorization' => 'Basic-of $client_id:x'), exchange({ 'client_secret' => 'x' }, 'Authorization' => 'Basic-of $client_id:'),
        exchange({ 'client_assertion' => 'a.b.c', 'client_assertion_type' => 'urn:ietf:params:oauth:client-assertion-type:jwt-bearer' }),
        exchange({ 'client_assertion' => 'a.b.c' }), exchange({ 'client_id' => 'nobody' }, 'Authorization' => 'Basic-of $client_id:'),
        exchange({ 'client_id' => nil }, 'Authorization' => 'Basic-of $client_id:'), refresh.merge('headers' => { 'Authorization' => 'Basic-of $client_id:' }),
        revoke('$access_token').merge('headers' => { 'Authorization' => 'Basic-of $client_id:' }), resolve('Bearer $access_token')
      ] },
      'token_confidential_client' => { 'sql' => [CONFIDENTIAL_SEED], 'steps' => sign_in + [
        authorize('client_id' => 'confidential-uid'), approve, exchange('client_id' => 'confidential-uid'),
        exchange('client_id' => 'confidential-uid', 'client_secret' => 'wrong'), exchange({ 'client_id' => nil }, 'Authorization' => 'Basic-of confidential-uid:wrong'),
        exchange({ 'client_id' => nil }, 'Authorization' => 'Basic-of confidential-uid:s3cret'),
        refresh('$refresh_token', 'client_id' => 'confidential-uid', 'client_secret' => 's3cret'),
        refresh('$refresh_token', 'client_id' => 'confidential-uid'), revoke('$access_token', 'client_id' => 'confidential-uid'),
        revoke('$access_token', 'client_id' => 'confidential-uid', 'client_secret' => 's3cret'), resolve('Bearer $access_token')
      ] },
      # A Basic value with a space in it is still a Basic header to Ruby's decoder: with a secret in the body that is two methods.
      'token_basic_with_whitespace' => { 'sql' => [CONFIDENTIAL_SEED], 'steps' => sign_in + [
        authorize('client_id' => 'confidential-uid'), approve,
        expect(400, exchange({ 'client_id' => 'confidential-uid', 'client_secret' => 's3cret' }, 'Authorization' => SPACED_BASIC)),
        expect(200, exchange({ 'client_id' => nil }, 'Authorization' => SPACED_BASIC)),
        expect(400, refresh('$refresh_token', 'client_id' => 'confidential-uid', 'client_secret' => 's3cret')
          .merge('headers' => { 'Authorization' => UNPADDED_BASIC })),
        expect(200, refresh('$refresh_token', 'client_id' => nil).merge('headers' => { 'Authorization' => "#{SPACED_BASIC}==" }))
      ] },
      # RFC 7636 4.1 wants 43 to 128 unreserved characters. Doorkeeper checks only that the hash matches.
      'token_verifier_outside_rfc_7636' => { 'steps' => [register] + sign_in + ['a', 'v' * 200, 'é' * 50, 'has space/and+signs='].flat_map do |verifier|
        [authorize('code_challenge' => Base64.urlsafe_encode64(Digest::SHA256.digest(verifier), padding: false)), approve, expect(200, exchange('code_verifier' => verifier))]
      end },
      'token_plain_grant_from_before' => { 'steps' => [register, with_sql(exchange('code' => 'plain-code'), grant_row('plain-code'))] },
      'token_personal_application' => { 'sql' => PERSONAL_SEED, 'steps' => [
        post_form('/oauth/token', { 'grant_type' => 'refresh_token', 'refresh_token' => PERSONAL_TOKEN, 'client_id' => 'personal-application-uid' }),
        post_form('/oauth/token', { 'grant_type' => 'authorization_code', 'code' => 'x', 'redirect_uri' => 'https://localhost/personal-access-token',
                                    'code_verifier' => VERIFIER, 'client_id' => 'personal-application-uid' })
      ] },
      'throttle_token' => { 'steps' => authorized + Array.new(20) { exchange('code' => 'nothing') } + [expect(429, exchange), later(30, expect(200, exchange))] },
      'unrouted_get_token' => { 'steps' => [get('/oauth/token')] }
    }
  end

  def refresh_scenarios
    {
      'refresh' => { 'steps' => connected + [later(10, expect(200, refresh)), resolve('Bearer $access_token.1'), resolve('Bearer $access_token.2')] },
      'refresh_after_the_access_token_expired' => { 'steps' => connected + [later(7200, expect(200, refresh)), resolve('Bearer $access_token.1')] },
      'refresh_narrower_scope' => { 'steps' => connected({ 'scope' => 'mcp api' }, { 'scope' => 'mcp api' }) + [
        refresh('$refresh_token', 'scope' => 'mcp'), refresh('$refresh_token', 'scope' => 'mcp api'), refresh('$refresh_token.1', 'scope' => 'api mcp'),
        refresh('$refresh_token.1')
      ] },
      'refresh_wider_scope' => { 'steps' => connected + [expect(400, refresh('$refresh_token', 'scope' => 'mcp api'))] },
      # Doorkeeper reads `scope`, and `scopes` only when no `scope` was sent at all.
      'refresh_scopes_alias' => { 'steps' => connected({ 'scope' => 'mcp api' }, { 'scope' => 'mcp api' }) + [
        expect(200, refresh('$refresh_token.1', 'scopes' => 'mcp')), expect(200, refresh('$refresh_token.1', 'scope' => 'api', 'scopes' => 'mcp')),
        expect(200, refresh('$refresh_token.1', 'scope' => '', 'scopes' => 'mcp')), expect(400, refresh('$refresh_token.1', 'scopes' => 'mcp admin')),
        resolve('Bearer $access_token.2', 'api'), resolve('Bearer $access_token.3', 'mcp'), resolve('Bearer $access_token.4', 'api')
      ] },
      'refresh_unknown_token' => { 'steps' => connected + [expect(400, refresh('nothing')), refresh('$access_token')] },
      'refresh_missing_parameters' => { 'steps' => connected + [refresh(nil), refresh('$refresh_token', 'client_id' => nil), refresh('', 'client_id' => '')] },
      'refresh_wrong_client' => { 'steps' => connected + [register, refresh('$refresh_token', 'client_id' => '$client_id.2'),
                                                           refresh('$refresh_token', 'client_id' => 'nobody')] },
      'refresh_revoked' => { 'steps' => connected + [revoke('$refresh_token'), expect(400, refresh)] },
      'refresh_lost_response_is_retried' => { 'steps' => connected + [later(10, refresh('$refresh_token.1')), later(1, expect(200, refresh('$refresh_token.1'))),
                                                                     use('Bearer $access_token.3'), later(1, expect(200, refresh('$refresh_token.3'))), use] },
      'refresh_keeps_a_longer_lifetime' => { 'steps' => [register, with_sql(refresh('two-hours-refresh'), token_row('two-hours', 'refresh_token' => 'two-hours-refresh',
                                                                                                                              'expires_in' => 7200))] },
      'refresh_chain' => { 'steps' => connected + [later(3600, refresh), use, later(3600, refresh), use, later(3600, refresh), use, later(3600, expect(200, refresh))] },
      'retired_refresh_token_after_the_new_access_token_was_used' => { 'steps' => connected + [
        later(10, expect(200, refresh('$refresh_token.1'))), use('Bearer $access_token.2'), later(1, refresh('$refresh_token.1'))
      ] },
      # Two refreshes and only the last access token used: both refresh tokens before it are retired, not the one before alone.
      'retired_every_ancestor' => { 'steps' => connected + [
        later(3600, expect(200, refresh)), later(3600, expect(200, refresh)), use('Bearer $access_token.3'), later(1, refresh('$refresh_token.1'))
      ] },
      # Two clients, each with a refreshed token. Using one client's new access token retires that client's old refresh
      # token and nothing of the other's.
      'retired_one_family_leaves_the_other' => { 'steps' => connected + [
        register, authorize('client_id' => '$client_id.2'), approve, expect(200, exchange('client_id' => '$client_id.2')),
        later(10, expect(200, refresh('$refresh_token.1', 'client_id' => '$client_id.1'))), expect(200, refresh('$refresh_token.2', 'client_id' => '$client_id.2')),
        use('Bearer $access_token.3'), later(1, expect(200, refresh('$refresh_token.2', 'client_id' => '$client_id.2'))),
        refresh('$refresh_token.1', 'client_id' => '$client_id.1')
      ] },
      # The handback: a refresh token this crate retired is refused by Rails, which reads `revoked_at` as it always did.
      # (Where Rails played the `use`, nothing was retired, and whoever plays the last step honours the token.)
      'retired_then_handed_back' => { 'steps' => connected + [later(10, expect(200, refresh)), cut(use), cut(later(1, refresh('$refresh_token.1')))] }
    }
  end

  def revocation
    {
      'revoke_access_token' => { 'steps' => connected + [expect(200, revoke('$access_token')), resolve('Bearer $access_token'), refresh] },
      'revoke_refresh_token' => { 'steps' => connected + [revoke('$refresh_token', 'token_type_hint' => 'refresh_token'), resolve('Bearer $access_token')] },
      'revoke_wrong_hint' => { 'steps' => connected + [revoke('$refresh_token', 'token_type_hint' => 'access_token'), resolve('Bearer $access_token')] },
      'revoke_twice_and_unknown' => { 'steps' => connected + [revoke('$access_token'), later(5, revoke('$access_token')), revoke('nothing'), revoke(nil)] },
      'revoke_without_client' => { 'steps' => connected + [expect(403, revoke('$access_token', 'client_id' => nil)), revoke('$access_token', 'client_id' => 'nobody'),
                                                             resolve('Bearer $access_token')] },
      'revoke_other_clients_token' => { 'steps' => connected + [register, expect(403, revoke('$access_token', 'client_id' => '$client_id.2')),
                                                                 resolve('Bearer $access_token')] },
      'revoke_from_a_browser' => { 'steps' => connected + from('main', [revoke('$access_token').merge('headers' => { 'Origin' => 'http://evil.example' })]) +
        [expect(302, get('/login'))] },
      'revoke_expired_access_token' => { 'steps' => connected + [later(3601, revoke('$access_token')), expect(200, refresh)] },
      'not_ported_introspect' => { 'steps' => connected + [post_form('/oauth/introspect', { 'token' => '$access_token', 'client_id' => '$client_id' })] },
      'not_ported_token_info' => { 'steps' => connected + [get('/oauth/token/info', nil, 'Authorization' => 'Bearer $access_token').merge('client' => 'api')] }
    }
  end

  def resolver
    live = 'live-token'
    {
      'resolve_header_forms' => { 'sql' => [token_row(live)], 'steps' => [
        resolve(nil), resolve(''), resolve('Bearer'), resolve('Bearer '), resolve("Bearer #{live}"), resolve("bearer #{live}"), resolve("BEARER   #{live}  "),
        resolve("Bearer\t#{live}"), resolve(" Bearer #{live}"), resolve("Bearer#{live}"), resolve("Basic #{live}"), resolve(live),
        resolve("Bearer #{live} extra"), resolve("Bearer #{live.upcase}"), resolve("Bearer #{'x' * 5000}"), resolve("Bearer #{' ' * 5000}"),
        resolve("Token #{live}"), resolve("Bearer #{live}", 'api'), resolve("Bearer #{live}", ''), resolve("Bearer #{live}", 'mc')
      ] },
      'resolve_token_states' => { 'sql' => PERSONAL_SEED + [
        token_row('both-scopes', 'scopes' => 'api mcp'), token_row('revoked', 'revoked_at' => '2026-09-10 12:00:30.123456'),
        token_row('revoked-later', 'revoked_at' => '2026-09-10 12:00:31'), token_row('revoked-and-expired', 'revoked_at' => '2026-09-10 12:00:00', 'expires_in' => 1,
                                                                                                           'created_at' => '2026-09-10 11:00:00'),
        token_row('expires-now', 'created_at' => '2026-09-10 11:00:30.123456'), token_row('expired', 'created_at' => '2026-09-10 11:00:30.123455'),
        token_row('never-expires', 'expires_in' => nil, 'created_at' => '2020-01-01 00:00:00'), token_row('no-scopes', 'scopes' => ''),
        token_row('no-user', 'resource_owner_id' => 999), token_row('null-user', 'resource_owner_id' => nil),
        token_row('expired-wrong-scope', 'scopes' => 'api', 'created_at' => '2026-09-10 10:00:00'), token_row('no-application', 'application_id' => 999)
      ], 'steps' => %w[both-scopes revoked revoked-later revoked-and-expired expires-now expired never-expires no-scopes no-user null-user expired-wrong-scope
                       no-application].map { |name| resolve("Bearer #{name}") } +
        [resolve('Bearer both-scopes', 'api'), resolve("Bearer #{PERSONAL_TOKEN}", 'api'), resolve("Bearer #{PERSONAL_TOKEN}"),
         later(1, resolve('Bearer revoked-later')), resolve('Bearer expires-now')] }
    }
  end

  def uri_rules
    registered = "https://client.example/callback\nhttp://127.0.0.1/cb\nhttp://localhost:3334/cb?x=1\nmyapp://callback\nhttp://[::1]/cb"
    urls = ['https://client.example/callback', 'https://client.example/callback/', 'https://client.example/callback?x=1', 'https://CLIENT.example/callback',
            'HTTPS://client.example/callback', 'https://client.example:443/callback', 'https://client.example/callback#f', 'https://client.example/call back',
            'http://127.0.0.1/cb', 'http://127.0.0.1:1/cb', 'http://127.0.0.2:9/cb', 'http://127.0.0.1:9/cb/', 'http://127.0.0.1:9/cb?x=1', 'https://127.0.0.1:9/cb',
            'http://user@127.0.0.1:9/cb', 'http://127.1:9/cb', 'http://localhost:9/cb?x=1', 'http://localhost:9/cb', 'http://LOCALHOST:9/cb?x=1',
            'http://[::1]:9/cb', 'http://[::1]/cb', 'http://[::2]:9/cb', 'http://[::ffff:127.0.0.1]:9/cb', 'http://0.0.0.0:9/cb', 'http://2130706433:9/cb',
            'myapp://callback', 'myapp://callback:9', 'javascript:alert(1)', 'data:text/html,x', 'urn:ietf:wg:oauth:2.0:oob', '/callback', 'callback', '',
            'https:///callback', 'https://client.example', 'http://127.0.0.1:x/cb', 'http://127.0.0.1:/cb', 'https://client.example/callback%2F',
            'https://client.example/callback%zz', "https://client.example/callback\n", 'https://client.example/cälback', 'https://client.example/callback?a[]=1']
    rewritten = "http://127.0.0.1/cb?x=hello%20world\nhttp://localhost/cb?x=hello world"
    more = ['http://127.0.0.1:9/cb?x=hello world', "http://127.0.0.1:9/cb?x=hello%20\tworld", 'http://127.0.0.1:9/cb?x=hello+world', "http://127.0.0.1:9/cb?x=hello\tworld",
            'http://localhost:9/cb?x=hello world', 'http://localhost:9/cb?x=hello', 'http://localhost:9/world', 'world', 'http://@127.0.0.1:9/cb?x=hello%20world',
            'HTTP://127.0.0.1:09/cb?x=hello%20world', 'http://127.0.0.1:9/cb?x=hello%20world#']
    { 'uri_rules' => { 'steps' => urls.map { |url| uri(url, registered) } + more.map { |url| uri(url, rewritten) } } }
  end

  # The scenarios with cuts: each is also played with Rails and Rust taking the legs in turns.
  def crossing
    again = [get('/login'), submit('/login', 'post', 'user[email]' => 'owner@example.com', 'user[password]' => Pages::PASSWORD)]
    {
      # A client connected under one runtime keeps working under the other, and back: its access token, its
      # refresh, the new token, a revocation.
      'cross_connect_refresh_and_use' => { 'steps' => connected + [use, cut(later(3000, use)), later(700, expect(200, refresh)), resolve('Bearer $access_token.1'), use,
                                                                 cut(later(3700, expect(200, refresh))), use, cut(revoke('$access_token')), resolve('Bearer $access_token'),
                                                                 expect(400, refresh)] },
      # An hourly refresh across four changes of runtime: every pair is issued by one and used by the other.
      'cross_refresh_chain' => { 'steps' => connected + [cut(later(3600, expect(200, refresh))), use, cut(later(3600, expect(200, refresh))), use,
                                                         cut(later(3600, expect(200, refresh))), use, cut(later(3600, expect(200, refresh))), use] },
      # A refresh whose response was lost, repeated after the change of runtime.
      'cross_lost_refresh_response' => { 'steps' => connected + [later(3600, refresh('$refresh_token.1')), cut(later(5, expect(200, refresh('$refresh_token.1')))),
                                                                 use, cut(later(3600, expect(200, refresh))), use] },
      # A code issued by one runtime is exchanged by the other, once.
      'cross_code_issued_before_the_switch' => { 'steps' => authorized + [cut(expect(200, exchange)), cut(expect(400, exchange)), expect(200, refresh)] },
      # A client registered under one runtime is authorised under the other and gets its tokens back under the first.
      'cross_registered_only' => { 'steps' => [register, cut(again[0]), again[1], authorize, approve, cut(expect(200, exchange)), use] },
      # The grant of tools written by one runtime is what the other shows and narrows at the next consent.
      'cross_reconsent' => { 'user' => control_enabled, 'steps' => connected + [
        cut(again[0]), again[1], expect(200, authorize), approve('granted_mcp_groups[]' => %w[read control]), expect(200, exchange),
        cut(again[0]), again[1], expect(200, authorize), deny, expect(200, refresh('$refresh_token.1')), use
      ] },
      # A token narrowed through `scopes` under one runtime stays narrow under the other.
      'cross_refresh_narrowed_by_the_alias' => { 'steps' => connected({ 'scope' => 'mcp api' }, { 'scope' => 'mcp api' }) + [
        cut(later(3600, expect(200, refresh('$refresh_token', 'scopes' => 'mcp')))), use, resolve('Bearer $access_token', 'api'),
        cut(later(3600, expect(200, refresh))), use, resolve('Bearer $access_token', 'api'), expect(400, refresh('$refresh_token', 'scope' => 'mcp api'))
      ] },
      'cross_revoked_stays_revoked' => { 'steps' => connected + [cut(revoke('$refresh_token')), cut(expect(400, refresh)), resolve('Bearer $access_token')] }
    }
  end

  def selected
    prefixes = ENV['OAUTH'].to_s.split(',')
    scenarios.select { |name, _| prefixes.empty? || prefixes.any? { |prefix| name.start_with?(prefix) } }
  end

  def legs(steps) = steps.slice_when { |_, after| after['cut'] }.to_a

  def runtime(mode, leg)
    case mode
    when 'rails', 'rust' then mode
    when 'rails_first' then leg.even? ? 'rails' : 'rust'
    else leg.even? ? 'rust' : 'rails'
    end
  end

  def grid(root)
    template = Pages.template(root)
    count = 0
    selected.each do |name, scenario|
      modes = legs(scenario['steps']).size > 1 ? MODES : MODES.first(2)
      modes.each do |mode|
        dir = File.join(root, mode, name)
        Pages.build(dir, template, scenario)
        Pages.connect(dir)
        scenario.fetch('sql', []).each { |sql| ActiveRecord::Base.connection.execute(sql) }
        ActiveRecord::Base.connection_pool.disconnect!
        File.write(File.join(dir, 'scenario.json'), JSON.pretty_generate(
                                                      'oauth_parity_scratch' => true, 'at' => Pages::AT,
                                                      'secret_key_base' => Rails.application.secret_key_base, 'steps' => scenario['steps'],
                                                      'allowed_hosts' => scenario['allowed_hosts']
                                                    ))
        File.write(File.join(dir, 'state.json'), JSON.generate('vars' => {}, 'elapsed' => 0))
        File.write(File.join(dir, 'transcript.json'), '[]')
        count += 1
      end
    end
    FileUtils.rm_rf(template)
    # The tool catalogue the consent screen and the grants are built from, for rust/src/web/consent.rs.
    File.write(File.join(root, 'catalogue.json'), JSON.pretty_generate('defaults' => AppConfig::MCP_TOOL_DEFAULTS, 'groups' => AppConfig::TOOL_GROUPS,
                                                                       'rest_defaults' => AppConfig::REST_TOOL_DEFAULTS))
    puts "built #{selected.size} scenarios (#{count} installs) in #{root}"
  end

  # "$name" in any string of a step is the value an earlier response gave: client_id, code,
  # access_token, refresh_token (the latest), or "$name.2" for the second one of the scenario.
  def fill(value, vars)
    case value
    when String then value.gsub(/\$([a-z_]+(?:\.\d+)?)/) { vars.fetch(Regexp.last_match(1)) { raise "no value for $#{Regexp.last_match(1)} yet" } }
    when Array then value.map { |item| fill(item, vars) }
    when Hash then value.transform_values { |item| fill(item, vars) }
    else value
    end
  end

  def remember(vars, name, value)
    return if value.blank? || vars.value?(value)

    number = 1 + vars.keys.count { |key| key.start_with?("#{name}.") }
    vars["#{name}.#{number}"] = vars[name] = value
  end

  CAPTURED = %w[client_id access_token refresh_token registration_access_token].freeze

  def capture(vars, response)
    if response.media_type == 'application/json'
      body = begin JSON.parse(response.body) rescue nil end
      CAPTURED.each { |name| remember(vars, name, body[name]) } if body.is_a?(Hash)
    end
    code = response.headers['location'].to_s[/[?&#]code=([^&#]+)/, 1]
    remember(vars, 'code', code)
  end

  # The fields a browser would send for the form the page has for `action` and `method`: its hidden
  # inputs, its ticked boxes and the button pressed. nil when the page has no such form.
  def form_fields(page, action, method)
    document = Nokogiri::HTML(page.to_s)
    form = document.css('form').find do |candidate|
      override = candidate.at_css('input[name="_method"]')&.[]('value') || candidate['method'] || 'get'
      candidate['action'] == action && override.casecmp?(method)
    end or return nil
    button = form.at_css('input[type="submit"]') || (form['id'] && document.at_css(%(input[type="submit"][form="#{form['id']}"])))
    form.css('input').to_a.push(button).compact.uniq.filter_map do |input|
      next unless input['name']
      next unless %w[hidden submit].include?(input['type']) || (input['type'] == 'checkbox' && input.key?('checked'))

      [input['name'], input['value'].to_s]
    end
  end

  def pairs(hash) = hash.flat_map { |name, value| Array(value).map { |one| [name, one.to_s] } }

  def play(root, leg)
    ActionController::Base.allow_forgery_protection = true # config/environments/test.rb turns it off
    Rack::Attack.enabled = true
    # The test environment re-raises; production answers. Answer as production's default does.
    Rails.application.env_config['action_dispatch.show_exceptions'] = :all
    Rails.application.env_config['action_dispatch.show_detailed_exceptions'] = false
    played = 0
    Dir[File.join(root, '*', '*', 'scenario.json')].each do |path|
      dir = File.dirname(path)
      scenario = JSON.parse(File.read(path))
      steps = legs(scenario['steps'])[leg]
      next unless steps && runtime(File.basename(File.dirname(dir)), leg) == 'rails'

      play_leg(dir, scenario, steps)
      played += 1
    end
    puts "Rails played leg #{leg} of #{played} installs"
  end

  # The app as production puts it together for this ALLOWED_HOSTS: config/environments/production.rb's own lines build
  # config.hosts, and ActionDispatch::HostAuthorization is then the first middleware, in front of everything else.
  # (The test environment has no such middleware; without the variable production has none either.)
  def guarded(allowed_hosts)
    lines = Rails.root.join('config/environments/production.rb').read[/^  if ENV\['ALLOWED_HOSTS'\]\.present\?\n.*?^  end\n/m] or
      raise 'production.rb no longer builds config.hosts from ALLOWED_HOSTS'
    config = Struct.new(:hosts).new([])
    kept = ENV.fetch('ALLOWED_HOSTS', nil)
    ENV['ALLOWED_HOSTS'] = allowed_hosts
    begin
      binding.eval(lines) # rubocop:disable Security/Eval
    ensure
      ENV['ALLOWED_HOSTS'] = kept
    end
    config.hosts.empty? ? Rails.application : ActionDispatch::HostAuthorization.new(Rails.application, config.hosts)
  end

  def play_leg(dir, scenario, steps)
    state = JSON.parse(File.read(File.join(dir, 'state.json')))
    transcript = JSON.parse(File.read(File.join(dir, 'transcript.json')))
    vars = state['vars']
    Pages.connect(dir)
    Rack::Attack.cache.store = ActiveSupport::Cache::MemoryStore.new
    app = guarded(scenario['allowed_hosts'])
    clients = Hash.new do |hash, name|
      session = ActionDispatch::Integration::Session.new(app)
      session.host! 'localhost:3000'
      hash[name] = { session:, page: nil, content: {} }
    end
    steps.each_with_index do |step, index|
      state['elapsed'] += step['advance'].to_i
      travel_to(Time.iso8601(scenario['at']) + state['elapsed'], with_usec: true) do
        step = fill(step, vars)
        Array(step['sql']).each { |sql| ActiveRecord::Base.connection.execute(sql) }
        answer = case step['do']
                 when 'resolve', 'use' then resolved(step)
                 when 'uri' then { 'allowed' => Doorkeeper::OAuth::Helpers::URIChecker.valid_for_authorization?(step['url'], step['registered']),
                                   'host' => (URI.parse(step['url']).host rescue nil) }
                 else requested("#{dir} step #{index}", clients, step, vars)
                 end
        transcript << answer.merge('by' => 'rails')
      end
    end
    travel_back
    ActiveRecord::Base.connection_pool.disconnect!
    File.write(File.join(dir, 'state.json'), JSON.generate(state))
    File.write(File.join(dir, 'transcript.json'), JSON.pretty_generate(transcript))
  end

  def resolved(step)
    result = OauthBearerTokenResolver.call(authorization_header: step['authorization'], required_scope: step['scope'])
    { 'resolved' => { 'error' => result.error&.to_s, 'user_id' => result.user&.id, 'application_id' => result.access_token&.application_id,
                      'token_id' => result.access_token&.id } }
  end

  # 'page_of' on a submit takes the form from another browser's last page: a form posted by somebody
  # who did not load it.
  def requested(where, clients, step, vars)
    client = clients[step['client'] || 'main']
    session = client[:session]
    headers = step.fetch('headers', {}).dup
    path = step['path'] || step['action']
    path += "?#{URI.encode_www_form(step['query'])}" if step['query']
    headers['Authorization'] = "Basic #{Base64.strict_encode64(Regexp.last_match(1))}" if headers['Authorization'].to_s =~ /\ABasic-of (.*)\z/
    headers['X-CSRF-Token'] = Pages.meta_token(client[:page]) if step['csrf'] == 'header'
    method = { 'get' => :get, 'head' => :head }.fetch(step['do'], :post)
    body = nil
    if step['do'] == 'submit'
      fields = form_fields(clients[step['page_of'] || step['client'] || 'main'][:page], step['action'], step['method']) or raise "#{where}: the last page has no #{step['method']} form for #{step['action']}"
      fields.reject! { |name, _| step['set'].key?(name) }
      body = URI.encode_www_form(fields + pairs(step['set'].compact))
      headers['Content-Type'] = 'application/x-www-form-urlencoded'
    elsif step['form']
      body = URI.encode_www_form(pairs(step['form']))
      headers['Content-Type'] ||= 'application/x-www-form-urlencoded'
    elsif step.key?('json')
      body = step['json'].is_a?(String) ? step['json'] : JSON.generate(step['json'])
      headers['Content-Type'] ||= 'application/json'
    end
    session.process(method, path, params: body, headers:)
    response = session.response
    Pages.verify_rendered!(where, session) if response.media_type == 'text/html' && session.controller && response.body.include?('authenticity_token')
    client[:page] = response.body if Pages.meta_token(response.body)
    capture(vars, response)
    # A request refused for its host never reached the session middleware.
    content = response.status == 403 && session.controller.nil? ? client[:content] : session.request.session.to_h.except('session_id')
    session_changed = content != client[:content]
    client[:content] = content
    { 'status' => response.status, 'headers' => response.headers.to_h.transform_keys(&:downcase), 'body' => response.body,
      'session_changed' => session_changed }
  end
end

command, argument, leg = ARGV
begin
  case command
  when 'grid' then OauthParity.grid(argument)
  when 'play' then OauthParity.play(argument, Integer(leg))
  else raise ArgumentError, 'usage: grid <root> | play <root> <leg>'
  end
rescue StandardError => e
  # Said here: the runner's own error reporter fails after an integration session has run.
  warn "#{e.class}: #{e.message}\n#{e.backtrace.first(8).join("\n")}"
  exit 1
end
