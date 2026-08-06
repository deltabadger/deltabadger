require 'test_helper'

# The rack-attack throttles key on a forwarded-for header only where something in front of
# the app writes one. Where nothing does, that header is caller-supplied and keying on it
# lets a caller pick a fresh bucket per request, which bounds nothing at all. This resolver
# is what tells the two cases apart, so the direction it errs in matters: an install wrongly
# read as proxied is back to an unbounded limit, while one wrongly read as direct keys on
# REMOTE_ADDR and is merely coarse.
class ProxyConfigurationTest < ActiveSupport::TestCase
  def behind_proxy_for(env)
    Deltabadger::Application.behind_proxy_from_env(env)
  end

  test 'declared by an explicit BEHIND_PROXY' do
    assert behind_proxy_for('BEHIND_PROXY' => 'true', 'APP_ROOT_URL' => 'http://localhost:3737')
  end

  test 'declared when the configured root URL is https' do
    assert behind_proxy_for('APP_ROOT_URL' => 'https://alice.deltabadger.com')
  end

  # This is the shipped self-hosted default: docker-compose publishes the container port
  # straight to the network with APP_ROOT_URL=http://localhost:3737 and nothing in front.
  test 'not declared for a plain-http self-hosted install' do
    refute behind_proxy_for('APP_ROOT_URL' => 'http://192.168.1.10:3737')
  end

  test 'not declared when nothing is configured' do
    refute behind_proxy_for({})
  end

  test 'an explicit BEHIND_PROXY=false wins over an https root URL' do
    refute behind_proxy_for('BEHIND_PROXY' => 'false', 'APP_ROOT_URL' => 'https://x.test')
  end

  # FORCE_SSL is the documented knob for a root URL that is not https (.env.docker.example),
  # which describes an operator who put a TLS proxy in front and left APP_ROOT_URL alone.
  # Nothing in this repo binds Puma to TLS, so SSL forced on top of a plain-http root URL
  # means the TLS is somebody else's and that somebody is in the request path.
  test 'declared by an explicit FORCE_SSL over a plain-http root URL' do
    assert behind_proxy_for('FORCE_SSL' => 'true', 'APP_ROOT_URL' => 'http://localhost:3737')
  end

  test 'an explicit BEHIND_PROXY wins over FORCE_SSL' do
    refute behind_proxy_for('BEHIND_PROXY' => 'no', 'FORCE_SSL' => 'true', 'APP_ROOT_URL' => 'https://x.test')
    assert behind_proxy_for('BEHIND_PROXY' => 'yes', 'FORCE_SSL' => 'false', 'APP_ROOT_URL' => 'http://x.test')
  end

  # Same list as FORCE_SSL, because the two settings are read by one another's helper and an
  # operator has no reason to expect them to accept different words.
  test 'an explicit BEHIND_PROXY accepts the usual spellings of on' do
    %w[true True TRUE 1 t y yes YES on ON].each do |value|
      assert behind_proxy_for('BEHIND_PROXY' => value, 'APP_ROOT_URL' => 'http://localhost:3737'),
             "BEHIND_PROXY=#{value} must declare a proxy"
    end
  end

  test 'an explicit BEHIND_PROXY accepts the usual spellings of off' do
    %w[false False FALSE 0 f n no NO off OFF].each do |value|
      refute behind_proxy_for('BEHIND_PROXY' => value, 'APP_ROOT_URL' => 'https://x.test'),
             "BEHIND_PROXY=#{value} must not declare a proxy"
    end
  end

  # An unrecognised value carries no intent to honour. Reading it as on is the direction that
  # costs something: it would hand the throttle key back to the caller on an install that
  # wrote a typo, so it falls through to what the deployment says about itself instead.
  test 'an unrecognised BEHIND_PROXY falls through to the deployment signals' do
    ['maybe', 'oui', '2', '-1', '', '   '].each do |value|
      assert behind_proxy_for('BEHIND_PROXY' => value, 'APP_ROOT_URL' => 'https://x.test'),
             "BEHIND_PROXY=#{value.inspect} must not override an https root URL"
      refute behind_proxy_for('BEHIND_PROXY' => value, 'APP_ROOT_URL' => 'http://x.test'),
             "BEHIND_PROXY=#{value.inspect} must not override a plain-http root URL"
    end
  end

  test 'the local environment resolves to no declared proxy' do
    refute Deltabadger::Application.behind_proxy_from_env,
           'a local run has nothing in front of it — check APP_ROOT_URL/FORCE_SSL/BEHIND_PROXY in .env'
  end
end
