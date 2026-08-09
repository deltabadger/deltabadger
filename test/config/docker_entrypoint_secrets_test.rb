# test/config/docker_entrypoint_secrets_test.rb
require 'test_helper'
require 'tmpdir'
require 'fileutils'
require 'open3'

# Drives the entrypoint's secret provisioning directly. SECRETS_FILE is overridable precisely
# so this is runnable outside a container.
class DockerEntrypointSecretsTest < ActiveSupport::TestCase
  SCRIPT = Rails.root.join('docker-entrypoint.sh')
  PRIMARY = 'ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY'.freeze
  SALT = 'ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT'.freeze
  MARKER = 'ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL'.freeze

  # Sources the script without running main, calls setup_secrets, and reports what the app
  # process would actually see in its environment.
  def run_setup(dir, env = {})
    command = <<~SH
      DELTABADGER_ENTRYPOINT_SOURCED=1
      export DELTABADGER_ENTRYPOINT_SOURCED
      . "#{SCRIPT}"
      setup_secrets 1>&2 || exit 1
      echo "SECRET_KEY_BASE=${SECRET_KEY_BASE}"
      echo "#{PRIMARY}=${#{PRIMARY}}"
      echo "#{SALT}=${#{SALT}}"
      echo "#{MARKER}=${#{MARKER}}"
    SH

    Open3.capture3(
      { 'SECRETS_FILE' => File.join(dir, '.secrets'),
        'DATABASE_PATH' => File.join(dir, 'production.sqlite3') }.merge(env),
      'bash', '-c', command
    )
  end

  def provision(dir, env = {})
    stdout, stderr, status = run_setup(dir, env)
    assert status.success?, "entrypoint failed: #{stderr}"
    stdout.lines.to_h { |line| line.strip.split('=', 2) }
  end

  def secrets_file(dir) = File.read(File.join(dir, '.secrets'))

  test 'a fresh install is provisioned with all three values' do
    Dir.mktmpdir do |dir|
      result = provision(dir)

      assert_equal 128, result['SECRET_KEY_BASE'].length
      assert_equal 64, result[PRIMARY].length
      assert_equal 64, result[SALT].length
      assert_not_equal result[PRIMARY], result[SALT]
    end
  end

  test 'the generated file is not world readable' do
    Dir.mktmpdir do |dir|
      provision(dir)

      assert_equal '600', format('%o', File.stat(File.join(dir, '.secrets')).mode & 0o777)
    end
  end

  # An install that already holds data was encrypted under keys derived from its
  # SECRET_KEY_BASE. Handing it new random ones makes all of that unreadable. This is
  # reachable without doing anything strange: restore a database, do not copy the hidden
  # secrets file, keep supplying SECRET_KEY_BASE from the environment.
  test 'an install with a database is never given generated encryption keys' do
    Dir.mktmpdir do |dir|
      File.write(File.join(dir, 'production.sqlite3'), 'not empty')

      result = provision(dir, 'SECRET_KEY_BASE' => 'preserved-from-the-environment')

      assert_equal '', result[PRIMARY]
      assert_equal '', result[SALT]
      assert_equal 'preserved-from-the-environment', result['SECRET_KEY_BASE']
    end
  end

  # A zero-byte database file is still a database. Treating it as absent would hand a
  # half-created install a random pair.
  test 'an empty database file still counts as a database' do
    Dir.mktmpdir do |dir|
      FileUtils.touch(File.join(dir, 'production.sqlite3'))

      assert_equal '', provision(dir)[PRIMARY]
    end
  end

  # A database supplied by URL is invisible to the path check, so the gate has to treat
  # either URL variable as an install that may already hold data.
  test 'a database supplied by URL is never given generated encryption keys' do
    %w[DATABASE_URL PRIMARY_DATABASE_URL].each do |variable|
      Dir.mktmpdir do |dir|
        result = provision(dir, variable => 'sqlite3:/restored/production.sqlite3')

        assert_equal '', result[PRIMARY], "#{variable} should have suppressed key generation"
        assert_equal '', result[SALT], "#{variable} should have suppressed key generation"
      end
    end
  end

  # The migration boundary. An existing install already has this file, so it is never
  # rewritten and never gains the new keys — which is what makes its data keep decrypting.
  test 'an existing secrets file is left exactly as it was' do
    Dir.mktmpdir do |dir|
      path = File.join(dir, '.secrets')
      File.write(path, "SECRET_KEY_BASE=existing-value\n")
      File.chmod(0o600, path)

      result = provision(dir)

      assert_equal "SECRET_KEY_BASE=existing-value\n", File.read(path)
      assert_equal 'existing-value', result['SECRET_KEY_BASE']
      assert_equal '', result[PRIMARY]
    end
  end

  # The key and its salt have to come from one source. Taking one from the environment and
  # the other from the file yields a mixed pair that reads nothing, and it would pass a
  # both-present check.
  test 'one key supplied externally does not pick up the other from the file' do
    Dir.mktmpdir do |dir|
      provision(dir)
      result = provision(dir, PRIMARY => 'supplied-externally')

      assert_equal 'supplied-externally', result[PRIMARY]
      assert_equal '', result[SALT], 'the salt must not be taken from the file'
    end
  end

  test 'both keys supplied externally win over the file' do
    Dir.mktmpdir do |dir|
      provision(dir)
      result = provision(dir, PRIMARY => 'external-primary', SALT => 'external-salt')

      assert_equal 'external-primary', result[PRIMARY]
      assert_equal 'external-salt', result[SALT]
    end
  end

  # The shell sees "   " as supplied; the app sees it as absent. Left disagreeing, the file
  # keys would be suppressed while the app fell back to deriving them, and stored data would
  # stop being readable.
  test 'a whitespace-only supplied key does not suppress the stored pair' do
    Dir.mktmpdir do |dir|
      generated = provision(dir)
      result = provision(dir, PRIMARY => '   ')

      assert_equal generated[PRIMARY], result[PRIMARY]
      assert_equal generated[SALT], result[SALT]
    end
  end

  # Writing a different pair into the file would leave a fallback that takes over silently if
  # the supplied environment is ever lost, and nothing written under the supplied keys would
  # read.
  test 'keys supplied externally on a first boot are not shadowed by a stored pair' do
    Dir.mktmpdir do |dir|
      provision(dir, PRIMARY => 'external-primary', SALT => 'external-salt')

      assert_not_includes secrets_file(dir), "#{PRIMARY}="
      assert_not_includes secrets_file(dir), "#{SALT}="
    end
  end

  # ...and the file records that they came from elsewhere, so a later boot without them stops
  # rather than deriving a different pair from the stored secret.
  test 'a first boot with external keys records that they are external' do
    Dir.mktmpdir do |dir|
      provision(dir, PRIMARY => 'external-primary', SALT => 'external-salt')

      assert_includes secrets_file(dir), "#{MARKER}=true"
      assert_equal 'true', provision(dir)[MARKER]
    end
  end

  # A secret supplied externally must not be shadowed either. Storing a different random one
  # leaves a value that is ignored while the environment supplies one and silently adopted
  # when it stops — and on an install that derives its encryption keys, adopting it makes
  # everything unreadable.
  test 'a secret supplied externally is not shadowed by a stored one' do
    Dir.mktmpdir do |dir|
      result = provision(dir, 'SECRET_KEY_BASE' => 'supplied-externally')

      assert_equal 'supplied-externally', result['SECRET_KEY_BASE']
      assert_not_includes secrets_file(dir), 'SECRET_KEY_BASE='
    end
  end

  # ...so losing it fails closed on the check the entrypoint already performs, rather than
  # starting on an unrelated value.
  test 'losing an externally supplied secret fails closed rather than inventing one' do
    Dir.mktmpdir do |dir|
      provision(dir, 'SECRET_KEY_BASE' => 'supplied-externally')

      stdout, stderr, status = run_setup(dir)

      assert_not status.success?, 'it should refuse to continue without a secret'
      assert_match(/SECRET_KEY_BASE not found/, stderr + stdout)
    end
  end

  test 'the file is read back when it already holds the new keys' do
    Dir.mktmpdir do |dir|
      assert_equal provision(dir), provision(dir)
    end
  end

  # Umbrel runs web and jobs against one shared volume. If they could each generate their own
  # keys the two halves would encrypt under different ones and neither could read what the
  # other wrote. Generation and adoption have to be a single atomic step.
  test 'concurrent first boots all adopt the same keys' do
    Dir.mktmpdir do |dir|
      command = <<~SH
        DELTABADGER_ENTRYPOINT_SOURCED=1
        export DELTABADGER_ENTRYPOINT_SOURCED
        . "#{SCRIPT}"
        for i in 1 2 3 4 5 6 7 8; do
          (
            setup_secrets >/dev/null 2>&1
            echo "${#{PRIMARY}}:${#{SALT}}"
          ) &
        done
        wait
      SH

      stdout, stderr, status = Open3.capture3(
        { 'SECRETS_FILE' => File.join(dir, '.secrets'),
          'DATABASE_PATH' => File.join(dir, 'production.sqlite3') }, 'bash', '-c', command
      )

      assert status.success?, "entrypoint failed: #{stderr}"
      adopted = stdout.split("\n").map(&:strip).reject(&:empty?)
      assert_equal 8, adopted.size
      assert_equal 1, adopted.uniq.size, "containers disagreed: #{adopted.uniq.inspect}"

      # And what they adopted is what is actually on disk, not merely consistent with each
      # other.
      primary, salt = adopted.first.split(':')
      assert_includes secrets_file(dir), "#{PRIMARY}=#{primary}"
      assert_includes secrets_file(dir), "#{SALT}=#{salt}"
      assert_equal 1, Dir.glob(File.join(dir, '.secrets*')).size, 'a temporary file was left behind'
    end
  end
end
