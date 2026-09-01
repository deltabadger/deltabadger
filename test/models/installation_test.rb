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
    assert_equal :hosted, platform_for('DELTABADGER_PLATFORM' => 'hosted')
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
  # for self-hosted installs pointing at their own. Reading it as "this is a hosted container"
  # would silently freeze those installs out of update notices forever.
  test 'MARKET_DATA_URL does not make an install hosted' do
    assert_equal :unknown, platform_for('MARKET_DATA_URL' => 'https://data.example.com')
  end

  test 'managed updates only where something else delivers them' do
    assert Installation.managed_updates?(:hosted)
    assert Installation.managed_updates?(:desktop)
    refute Installation.managed_updates?(:docker)
    refute Installation.managed_updates?(:umbrel)
    refute Installation.managed_updates?(:unknown)
  end

  test 'platform reads the real environment' do
    original = ENV['DELTABADGER_PLATFORM']
    ENV['DELTABADGER_PLATFORM'] = 'docker'
    assert_equal :docker, Installation.platform
  ensure
    ENV['DELTABADGER_PLATFORM'] = original
  end
end
