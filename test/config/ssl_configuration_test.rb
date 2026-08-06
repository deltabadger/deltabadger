require 'test_helper'
require 'tmpdir'

# SSL used to hang off FORCE_SSL alone. Nothing in the shipped deployment tooling sets it,
# so https deployments ran with no HSTS and no Secure flag on a 30-day session cookie.
# It is now derived from the configured root URL, with FORCE_SSL kept as an explicit
# override in both directions.
class SslConfigurationTest < ActiveSupport::TestCase
  def force_ssl_for(env)
    Deltabadger::Application.force_ssl_from_env(env)
  end

  test 'enabled by an explicit FORCE_SSL' do
    assert force_ssl_for('FORCE_SSL' => 'true', 'APP_ROOT_URL' => 'http://localhost:3000')
  end

  test 'enabled when the configured root URL is https' do
    assert force_ssl_for('APP_ROOT_URL' => 'https://alice.deltabadger.com')
  end

  test 'disabled for a plain-http self-hosted install' do
    refute force_ssl_for('APP_ROOT_URL' => 'http://192.168.1.10:3737')
  end

  test 'disabled when nothing is configured' do
    refute force_ssl_for({})
  end

  test 'an explicit FORCE_SSL=false wins over an https root URL' do
    refute force_ssl_for('FORCE_SSL' => 'false', 'APP_ROOT_URL' => 'https://x.test')
  end

  # An operator who writes FORCE_SSL=1 means "on". Comparing against the literal string
  # 'true' reads every other spelling as "explicitly off" and silently downgrades an https
  # deployment — the exact opposite of what was asked for.
  test 'an explicit FORCE_SSL accepts the usual spellings of on' do
    %w[true True TRUE 1 t y yes YES on ON].each do |value|
      assert force_ssl_for('FORCE_SSL' => value, 'APP_ROOT_URL' => 'http://localhost:3000'),
             "FORCE_SSL=#{value} must enable SSL"
    end
  end

  test 'an explicit FORCE_SSL accepts the usual spellings of off' do
    %w[false False FALSE 0 f n no NO off OFF].each do |value|
      refute force_ssl_for('FORCE_SSL' => value, 'APP_ROOT_URL' => 'https://x.test'),
             "FORCE_SSL=#{value} must disable SSL"
    end
  end

  # 'no' is the one that bites. It is missing from Rails' boolean FALSE_VALUES, so a cast
  # reads it as on — and on a plain-http install that is not a degraded site but a dead
  # one: with the connection assumed secure, every redirect and every *_url helper points
  # at https on a host with no TLS listener, so the browser cannot connect at all.
  test 'an explicit FORCE_SSL=no keeps a plain-http install on plain http' do
    %w[no No NO n].each do |value|
      refute force_ssl_for('FORCE_SSL' => value, 'APP_ROOT_URL' => 'http://192.168.1.10:3737'),
             "FORCE_SSL=#{value} must not turn SSL on"
    end
  end

  # An unrecognised value carries no intent to honour, so it must not decide either way.
  # It falls through to the root URL — the secure-by-default branch. Reading it as false
  # would be the mirror image of the bug above.
  test 'an unrecognised FORCE_SSL falls through to the root URL' do
    %w[maybe oui 2 -1].each do |value|
      assert force_ssl_for('FORCE_SSL' => value, 'APP_ROOT_URL' => 'https://x.test'),
             "FORCE_SSL=#{value} must not override an https root URL"
      refute force_ssl_for('FORCE_SSL' => value, 'APP_ROOT_URL' => 'http://x.test'),
             "FORCE_SSL=#{value} must not override a plain-http root URL"
    end
  end

  test 'a blank FORCE_SSL falls through to the root URL' do
    ['', '   '].each do |value|
      assert force_ssl_for('FORCE_SSL' => value, 'APP_ROOT_URL' => 'https://x.test'),
             "FORCE_SSL=#{value.inspect} must not override an https root URL"
      refute force_ssl_for('FORCE_SSL' => value, 'APP_ROOT_URL' => 'http://x.test'),
             "FORCE_SSL=#{value.inspect} must not override a plain-http root URL"
    end
  end

  # Rack::Test will not replay a Secure cookie over http, so an https root URL in a local
  # .env turns into a pile of unexplained session failures elsewhere in the suite. Fail
  # here instead, where the cause is named.
  test 'the local environment resolves to plain http' do
    refute Deltabadger::Application.force_ssl_from_env,
           'a local run must not force SSL — check APP_ROOT_URL/FORCE_SSL in .env'
  end

  # The resolver passing says nothing about whether production consumes it: the resolver,
  # the production.rb assignment and the cookie options are three separate edits. Boot a
  # real production environment and read the end state.
  test 'a production boot with an https root URL forces SSL and secures the cookie' do
    config = boot_production('APP_ROOT_URL' => 'https://x.test')

    assert config['force_ssl'], 'production with an https APP_ROOT_URL must force SSL'
    assert config['secure'], 'the session cookie must carry Secure'
    # secure is set by the middleware stack on its own whenever force_ssl is on, so it
    # cannot show whether the cookie options were written. same_site can: left alone it
    # is a per-request Proc, which serialises to {}.
    assert_equal 'lax', config['same_site'],
                 'the cookie options must be written explicitly, not left to the default'
  end

  # force_ssl answers any request it believes is plaintext with a 301, and both health
  # endpoints are exactly that — the container's own HEALTHCHECK curl and the routing
  # proxy's probe reach the app over plain http with no forwarded-proto header. Neither
  # notices: `curl -fsS` exits 0 on a 301, so the container reports healthy while the
  # proxy refuses to route to it. assume_ssl is what keeps them at 200.
  test 'a production boot leaves the health endpoints unredirected' do
    config = boot_production('APP_ROOT_URL' => 'https://x.test')

    assert_equal 200, config['up_status'],
                 '/up must answer a plain-http probe with 200, not a redirect'
    assert_equal 200, config['health_check_status'],
                 '/health-check must answer a plain-http probe with 200, not a redirect'
    assert config['assume_ssl'], 'assume_ssl must be enabled alongside force_ssl'
  end

  # A plain-http self-hosted install must be untouched — a Secure cookie over http locks
  # the owner out of their own instance.
  test 'a production boot with a plain-http root URL leaves the connection alone' do
    config = boot_production('APP_ROOT_URL' => 'http://192.168.1.10:3737')

    refute config['force_ssl']
    refute config['assume_ssl']
    refute config['secure']
  end

  private

  def boot_production(env)
    script = <<~'RUBY'
      require ENV.fetch('BOOT_FILE')
      require 'rack/mock'

      app = Rails.application
      options = app.config.session_options
      status = ->(path) { app.call(Rack::MockRequest.env_for("http://x.test#{path}")).first }

      puts "SSL_CONFIG_JSON#{{
        force_ssl: app.config.force_ssl,
        assume_ssl: app.config.assume_ssl,
        secure: options[:secure],
        same_site: options[:same_site],
        up_status: status.call('/up'),
        health_check_status: status.call('/health-check')
      }.to_json}"
    RUBY

    output = Dir.mktmpdir do |dir|
      IO.popen(
        boot_env(dir).merge(env), ['ruby', '-e', script],
        chdir: Rails.root.to_s, err: %i[child out], &:read
      )
    end

    json = output[/^SSL_CONFIG_JSON(.*)$/, 1]
    assert json, "the production boot reported no configuration:\n#{output}"
    JSON.parse(json)
  end

  # A nil value is how IO.popen unsets an inherited variable: SECRET_KEY_BASE_DUMMY is
  # consulted ahead of SECRET_KEY_BASE, so an inherited one would win over the real key.
  def boot_env(dir)
    { 'RAILS_ENV' => 'production',
      'BOOT_FILE' => Rails.root.join('config/environment').to_s,
      'RAILS_LOG_TO_STDOUT' => 'true',
      'SECRET_KEY_BASE' => SecureRandom.hex(64),
      'SECRET_KEY_BASE_DUMMY' => nil,
      'DATABASE_PATH' => File.join(dir, 'production.sqlite3'),
      'QUEUE_DATABASE_PATH' => File.join(dir, 'production_queue.sqlite3'),
      'CACHE_DATABASE_PATH' => File.join(dir, 'production_cache.sqlite3'),
      'CABLE_DATABASE_PATH' => File.join(dir, 'production_cable.sqlite3') }
  end
end
