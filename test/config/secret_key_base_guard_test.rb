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

  # deltabadger/docker-compose.yml sets "${APP_SEED}-secret-key-base". Under Umbrel
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

  # The step that blanks SECRET_KEY_BASE edits .env.docker, and docker-compose.yml loads it
  # through `env_file`, which Compose resolves when the container is CREATED. So
  # `docker compose start` — the natural pair for the `docker compose stop` this procedure
  # opens with — brings the OLD container back up with its baked-in environment. The
  # published key stays live, and every credential the operator reissues afterwards is
  # encrypted under it again, which is the one outcome this whole procedure exists to
  # prevent. The restart has to recreate the container.
  #
  # This cannot be fixed by opening with `docker compose down` instead: the backup step
  # copies out with `docker compose cp`, which needs the container to still exist. Stop,
  # copy, recreate at the end is the only order that works, so the ordering is pinned here
  # too — otherwise the obvious fix to the first assertion silently breaks the backup.
  #
  # Unverified against a running stack: docker is not available in this environment, so this
  # rests on Compose's documented env_file-at-create-time behaviour, not on an observed run.
  test 'the compromised procedure recreates the container rather than restarting it' do
    warning = SecretKeyBaseGuard::PUBLISHED_WARNING

    assert_match(/docker compose up -d --force-recreate/, warning)
    assert_no_match(/docker compose start/, warning)
    assert_includes warning, 'docker compose stop',
                    'the app must be stopped, not removed — the backup copies out of the container'
    assert_operator warning.index('docker compose stop'), :<, warning.index('docker compose cp'),
                    'the backup copies out of the stopped container, so it must still exist'
    assert_operator warning.index('docker compose cp'), :<, warning.index('--force-recreate'),
                    'recreating discards the container the backup reads'
  end

  # README.md carries the same procedure for the same operator; the warning is only what
  # someone who never opens the README sees. Both are edited by hand and have drifted before.
  test 'the README procedure recreates the container too' do
    section = readme_recovery_section

    assert section, 'expected the recovery section in README.md'
    assert_match(/docker compose up -d --force-recreate/, section)
    assert_no_match(/docker compose start/, section)
  end

  # Blanking the env file is only half a rotation, and on the documented default it is none
  # of one. .env.docker ships EMPTY: the entrypoint generates a key into
  # /app/storage/.secrets on first boot, and load_secrets falls back to that file whenever
  # the environment is empty. generate_secrets returns early when the file already exists,
  # so it is never overwritten — deleting it is the only thing that rotates a default
  # install. Without this the operator blanks an already-blank line, recreates the container
  # onto the identical key, and has destroyed every credential to arrive back where they
  # started, having waited days for IBKR on the way.
  #
  # The entrypoint is read rather than trusted so that if that reuse behaviour ever changes,
  # this fails and the prose gets revisited instead of quietly going stale.
  test 'the compromised procedure rotates the generated key file, not only the env file' do
    generate = File.read(Rails.root.join('docker-entrypoint.sh'))[/^generate_secrets\(\).*?\n\}/m]

    assert generate, 'expected a generate_secrets function in docker-entrypoint.sh'
    assert_match(/if \[ -f "\$SECRETS_FILE" \]/, generate,
                 'this test exists because an existing .secrets is reused, never regenerated')

    warning = SecretKeyBaseGuard::PUBLISHED_WARNING
    assert_match(%r{/app/storage/\.secrets}, warning)
    assert_match(%r{/app/storage/\.secrets}, readme_recovery_section)

    # Deleting it before the report would take the withdrawal addresses with it — the report
    # is the one step that reads encrypted values back, and it needs the old key to do it.
    # Anchored on the deletion, not on any mention: the backup step names the same path, and
    # that one legitimately comes first.
    assert_includes warning, 'rm -f /app/storage/.secrets'
    assert_operator warning.index('encryption:report'), :<, warning.index('rm -f /app/storage/.secrets'),
                    'the report reads withdrawal addresses back under the old key'
  end

  # The backup copies /app/storage to the operator's host, and .secrets lives inside it, so
  # the copy carries its own decryption key. An operator not told that will reasonably drop
  # it in a synced folder or attach it to a support thread — the branch's own threat model
  # says a database plus its key is total credential disclosure.
  test 'the compromised procedure says the backup carries the key with it' do
    backup_step = SecretKeyBaseGuard::PUBLISHED_WARNING[/2\. Back up.*?(?=  3\.)/m]

    assert backup_step, 'expected the backup step in the warning'
    assert_match(/\.secrets/, backup_step, 'the operator must be told the backup decrypts itself')
    assert_match(/\.secrets/, readme_recovery_section[/2\. Back up.*?(?=\n3\.)/m].to_s,
                 'the README backup step must say so too')
  end

  # deltabadger/docker-compose.yml is the Umbrel app definition, and it supplies the secret
  # through `environment:` from APP_SEED — that deployment has no .env.docker at all. Run it
  # outside Umbrel and APP_SEED is unset, so the value expands to the bare "-secret-key-base"
  # this guard treats as published: exactly that operator gets PUBLISHED_WARNING, and it
  # tells them to blank a line in a file they do not have. Same defect as the rest of this
  # procedure was fixed for — a step that cannot be performed.
  #
  # Read from the compose file rather than hardcoded, so that if the Umbrel definition is
  # ever dropped this test is what says the paragraph can go with it.
  test 'the compromised procedure covers a secret supplied outside .env.docker' do
    compose = File.read(Rails.root.join('deltabadger/docker-compose.yml'))

    assert_match(/SECRET_KEY_BASE:\s*\$\{APP_SEED\}/, compose,
                 'this test exists because the Umbrel definition sets the secret by environment')
    assert_match(/APP_SEED/, SecretKeyBaseGuard::PUBLISHED_WARNING,
                 'an operator whose secret comes from APP_SEED must be told where to change it')

    # Where the secret lives is only half of it. Every command in the procedure names the
    # service from docker-compose.yml, and this file does not have a service by that name,
    # so the backup, the report and the reset are all unrunnable as written for the same
    # operator. Fixing only step 6 would leave them stuck three steps earlier.
    assert_match(/^  web:$/, compose, 'the Umbrel definition calls its app service web')
    assert_match(/\bweb\b/, SecretKeyBaseGuard::PUBLISHED_WARNING,
                 'the commands name the docker-compose.yml service; this operator must be told theirs differs')
  end

  # short? is length alone, and length alone distinguishes nothing: a randomly generated
  # 31-character secret and the word "password1" both land here. So this warning has to hold
  # two lines at once, and each assertion below pins one of them.
  #
  # Under-warning is the expensive failure. A human-chosen key is cheap to brute-force from
  # one encrypted database value or a captured session cookie, which yields every stored
  # credential and a forgeable session — so the warning must name the condition under which
  # this operator should treat their secret as compromised and revoke. It previously claimed
  # the opposite outright ("This is not an exposure ... there is no urgency"), which is a
  # statement about the value's provenance that nothing in the value supports.
  #
  # Over-warning still has a real cost, which is why the flat compromise language stays
  # barred here: it would send an operator whose secret genuinely never left the machine
  # through a recovery that destroys IBKR credentials taking days to replace.
  test 'the short-secret warning neither claims the data has leaked nor claims there is nothing to do' do
    warning = SecretKeyBaseGuard.warning_for("#{SecureRandom.hex(15)}a")

    assert warning, 'a short secret must still warn'
    assert_not_equal SecretKeyBaseGuard::PUBLISHED_WARNING, warning
    assert_no_match(/anyone with a copy/i, warning)
    assert_no_match(/can already read/i, warning)
    assert_no_match(/no urgency/i, warning)
    assert_no_match(/nothing to revoke/i, warning)
    assert_match(/compromised/i, warning)
    assert_match(/revok/i, warning) # revoke / revoking
  end

  test 'a strong secret produces no warning' do
    assert_nil SecretKeyBaseGuard.warning_for(SecureRandom.hex(64))
  end

  # The four tests above exercise the selector. These exercise what production actually
  # runs: without them the boot branch could regress to weak? + PUBLISHED_WARNING and every
  # test above would still pass, reinstating the exact harmful message for short secrets.
  test 'a short secret emits the short-secret warning at boot, not the compromise one' do
    _out, err = capture_io { SecretKeyBaseGuard.emit_warning!("#{SecureRandom.hex(15)}a", logger: nil) }

    assert_equal SecretKeyBaseGuard::SHORT_WARNING, err
    assert_no_match(/anyone with a copy/i, err)
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

  private

  # Defined after the tests on purpose: `private` applies to what follows it, and the test
  # helper defines its blocks as methods, so putting this earlier would make them private
  # and silently unrunnable.
  def readme_recovery_section
    File.read(Rails.root.join('README.md'))[/### Moving to a new SECRET_KEY_BASE.*?\n### /m]
  end
end
