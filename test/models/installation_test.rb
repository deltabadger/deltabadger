require 'test_helper'

# How a copy of Deltabadger was installed decides how it is updated, and the answer is never
# guessed. Every signal available here — /.dockerenv, MARKET_DATA_URL — proves something other
# than what it would be used to claim, and both ways of being wrong hurt: an install told it is
# managed never hears about an update, and an install told it is Compose is handed a command that
# cannot work where it runs.
class InstallationTest < ActiveSupport::TestCase
  def platform_for(env) = Installation.platform_from_env(env)

  test 'each marker names its platform' do
    assert_equal :docker, platform_for('DELTABADGER_PLATFORM' => 'docker')
    assert_equal :umbrel, platform_for('DELTABADGER_PLATFORM' => 'umbrel')
    assert_equal :desktop, platform_for('DELTABADGER_PLATFORM' => 'desktop')
  end

  test 'a marker is read regardless of case and surrounding space' do
    assert_equal :umbrel, platform_for('DELTABADGER_PLATFORM' => '  Umbrel ')
    assert_equal :docker, platform_for('DELTABADGER_PLATFORM' => 'DOCKER')
  end

  test 'an unrecognised marker is unknown, not the raw value' do
    assert_equal :unknown, platform_for('DELTABADGER_PLATFORM' => 'kubernetes')
  end

  test 'no marker is unknown' do
    assert_equal :unknown, platform_for({})
    assert_equal :unknown, platform_for('DELTABADGER_PLATFORM' => '   ')
  end

  # MARKET_DATA_URL names any separately managed market-data service — the manual documents it
  # for self-hosted installs pointing at their own, so it says nothing about how this copy was
  # installed. Reading it as a platform would freeze those installs out of update notices forever.
  test 'MARKET_DATA_URL names no platform' do
    assert_equal :unknown, platform_for('MARKET_DATA_URL' => 'https://data.example.com')
  end

  test 'only the desktop build installs updates itself' do
    assert Installation.desktop?(:desktop)
    refute Installation.desktop?(:docker)
    refute Installation.desktop?(:umbrel)
    refute Installation.desktop?(:unknown)
  end

  test 'platform reads the real environment' do
    original = ENV['DELTABADGER_PLATFORM']
    ENV['DELTABADGER_PLATFORM'] = 'docker'
    assert_equal :docker, Installation.platform
  ensure
    ENV['DELTABADGER_PLATFORM'] = original
  end
end
