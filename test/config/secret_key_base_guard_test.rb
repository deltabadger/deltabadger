# test/config/secret_key_base_guard_test.rb
require 'test_helper'

class SecretKeyBaseGuardTest < ActiveSupport::TestCase
  test 'rejects the value shipped in .env.docker.example' do
    assert SecretKeyBaseGuard.weak?('dev-secret-key-not-for-production')
  end

  test 'rejects the Docker build placeholder' do
    assert SecretKeyBaseGuard.weak?('placeholder')
  end

  # Every literal SECRET_KEY_BASE value this repository has ever shipped, from a sweep of
  # every blob in history. All three must be published?, not merely weak?: at 25 characters
  # "your_secret_key_base_here" is under MINIMUM_LENGTH, so without an entry it would be
  # classified as a privately generated short secret and its operator told nothing leaked.
  test 'treats every literal secret this repository has ever shipped as published' do
    %w[
      dev-secret-key-not-for-production
      placeholder
      your_secret_key_base_here
    ].each do |value|
      assert SecretKeyBaseGuard.published?(value), "#{value.inspect} was shipped in this repository"
    end
  end

  # umbrel/deltabadger/docker-compose.yml sets "${APP_SEED}-secret-key-base". Under Umbrel
  # APP_SEED is a per-device secret and the result is fine; run anywhere else APP_SEED is
  # unset and compose expands it to this bare literal, which is published here verbatim.
  test 'treats the Umbrel template expanded with an empty APP_SEED as published' do
    assert SecretKeyBaseGuard.published?('-secret-key-base')
    assert_not SecretKeyBaseGuard.published?("#{SecureRandom.hex(32)}-secret-key-base")
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
  # A substring match on "secret" would abort every Umbrel container on boot — it is a
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

  # The Docker asset-precompile step must not regress to a real-looking value: the guard
  # only warns (never aborts) in production, so a reintroduced `SECRET_KEY_BASE=placeholder`
  # would still build successfully and silently — nobody reads precompile logs for this.
  test 'the Dockerfile precompile step uses the dummy secret flag, not a real value' do
    line = File.readlines(Rails.root.join('Dockerfile'))
               .find { |l| l =~ /^RUN SECRET_KEY_BASE/ }

    assert line, 'expected the asset-precompile RUN line in Dockerfile'
    assert_equal "RUN SECRET_KEY_BASE_DUMMY=1 \\\n", line,
                 'asset precompile must use SECRET_KEY_BASE_DUMMY, not a real SECRET_KEY_BASE value'
  end

  # The credential reset runs against a STOPPED stack via `docker compose run`, which means
  # it does not inherit a loaded environment. The catch-all `*)` branch execs the command
  # directly without calling setup_secrets, so an install whose secret lives in
  # /app/storage/.secrets rather than .env.docker would fail to boot. This case exists to
  # load them first.
  test 'the entrypoint runs one-shot rails commands with secrets loaded' do
    script = File.read(Rails.root.join('docker-entrypoint.sh'))
    branch = script[/^\s*rake\).*?;;/m]

    assert branch, 'expected a `rake)` case in docker-entrypoint.sh'
    assert_includes branch, 'setup_secrets'
    assert_includes branch, 'exec bundle exec rails "$@"'

    # Without `shift`, "$@" still starts with the literal word "rake", so rails would
    # receive "rake" as its first argument instead of the real task name. No container
    # has ever run this branch to catch that in practice — docker is unavailable in this
    # environment — so this line is the only thing standing between a missing `shift` and
    # every one-shot rake invocation silently doing the wrong thing.
    assert_includes branch, 'shift'
    assert_operator branch.index('setup_secrets'), :<, branch.index('exec bundle exec rails "$@"'),
                    'setup_secrets must run before exec, or the secrets it loads never reach rails'
  end

  test 'a published placeholder is treated as compromised' do
    assert SecretKeyBaseGuard.published?('dev-secret-key-not-for-production')
    assert SecretKeyBaseGuard.weak?('dev-secret-key-not-for-production')
  end

  # 31 random characters is weaker than we want, but it was never published. Telling this
  # operator their database is readable by anyone would be false, and the recovery it
  # points at destroys IBKR credentials that take days to replace.
  test 'a short but private secret is not treated as compromised' do
    secret = "#{SecureRandom.hex(15)}a" # 31 characters

    assert SecretKeyBaseGuard.short?(secret)
    assert SecretKeyBaseGuard.weak?(secret)
    assert_not SecretKeyBaseGuard.published?(secret)
  end

  test 'the compromised warning tells the operator to revoke' do
    warning = SecretKeyBaseGuard.warning_for('dev-secret-key-not-for-production')

    assert_match(/revoke/i, warning)
  end

  test 'the short-secret warning does not tell the operator to revoke' do
    warning = SecretKeyBaseGuard.warning_for("#{SecureRandom.hex(15)}a")

    assert warning, 'a short secret must still warn'
    assert_no_match(/revoke/i, warning)
    assert_no_match(/anyone/i, warning)
  end

  test 'a strong secret produces no warning' do
    assert_nil SecretKeyBaseGuard.warning_for(SecureRandom.hex(64))
  end

  # The four tests above exercise the selector. These exercise what production actually
  # runs: without them the boot branch could regress to weak? + PUBLISHED_WARNING and every
  # test above would still pass, reinstating the exact harmful message for short secrets.
  test 'a short private secret emits the non-incident warning at boot' do
    _out, err = capture_io { SecretKeyBaseGuard.emit_warning!("#{SecureRandom.hex(15)}a", logger: nil) }

    assert_equal SecretKeyBaseGuard::SHORT_WARNING, err
    assert_no_match(/revoke/i, err)
    assert_no_match(/anyone/i, err)
  end

  test 'the published secret emits the compromise warning at boot' do
    _out, err = capture_io do
      SecretKeyBaseGuard.emit_warning!('dev-secret-key-not-for-production', logger: nil)
    end

    assert_equal SecretKeyBaseGuard::PUBLISHED_WARNING, err
    assert_match(/revoke/i, err)
  end

  test 'a strong secret emits nothing at boot' do
    _out, err = capture_io { SecretKeyBaseGuard.emit_warning!(SecureRandom.hex(64), logger: nil) }

    assert_empty err
  end

  # emit_warning! is only worth testing if the initializer actually calls it. Same
  # file-content approach as the Dockerfile assertion above.
  test 'the production boot branch goes through emit_warning!' do
    source = File.read(Rails.root.join('config/initializers/secret_key_base_guard.rb'))
    branch = source[/if Rails\.env\.production\?.*\Z/m]

    assert branch, 'expected a production boot branch in the initializer'
    assert_includes branch, 'SecretKeyBaseGuard.emit_warning!'
  end
end
