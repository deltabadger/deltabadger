# test/config/secret_key_base_guard_test.rb
require 'test_helper'

class SecretKeyBaseGuardTest < ActiveSupport::TestCase
  test 'rejects the value shipped in .env.docker.example' do
    assert SecretKeyBaseGuard.weak?('dev-secret-key-not-for-production')
  end

  test 'rejects the Docker build placeholder' do
    assert SecretKeyBaseGuard.weak?('placeholder')
  end

  test 'rejects anything shorter than 32 characters' do
    assert SecretKeyBaseGuard.weak?('a' * 31)
  end

  test 'rejects blank' do
    assert SecretKeyBaseGuard.weak?(nil)
    assert SecretKeyBaseGuard.weak?('')
  end

  test 'accepts a real generated secret' do
    refute SecretKeyBaseGuard.weak?(SecureRandom.hex(64))
  end

  # deltabadger/docker-compose.yml:24,44 sets SECRET_KEY_BASE to "${APP_SEED}-secret-key-base".
  # A substring match on "secret" would abort every Umbrel container on boot. The audit
  # specifically REFUTED the claim that this value degrades to a constant — it is a
  # legitimate per-device secret and must keep working.
  test 'accepts the Umbrel APP_SEED-derived secret' do
    refute SecretKeyBaseGuard.weak?("#{SecureRandom.hex(32)}-secret-key-base")
  end

  test 'matches placeholders exactly, not as substrings' do
    refute SecretKeyBaseGuard.weak?("#{SecureRandom.hex(32)}-placeholder-suffix")
  end

  # The .env.docker.example file must not regress into shipping a live value.
  test 'the shipped example file does not set a usable secret' do
    line = File.readlines(Rails.root.join('.env.docker.example'))
               .find { |l| l.start_with?('SECRET_KEY_BASE=') }

    assert line, 'expected a SECRET_KEY_BASE line in .env.docker.example'
    assert_equal 'SECRET_KEY_BASE=', line.strip,
                 'the example file must ship an EMPTY secret so the entrypoint generates one'
  end
end
