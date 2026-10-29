require 'test_helper'
require 'open3'
require 'tmpdir'

class DockerEntrypointExportTest < ActiveSupport::TestCase
  test 'root prepares the export mount before dropping privileges' do
    Dir.mktmpdir do |dir|
      commands = { 'id' => 'echo 0', 'bundle' => 'exit 0',
                   'mkdir' => 'echo "mkdir:$*"', 'chown' => 'echo "chown:$*"', 'gosu' => 'echo "gosu:$*"' }
      commands.each do |name, body|
        path = File.join(dir, name)
        File.write(path, "#!/bin/sh\n#{body}\n")
        FileUtils.chmod(0o755, path)
      end
      env = { 'PATH' => "#{dir}:#{ENV.fetch('PATH')}", 'EXPORT_SCRATCH' => '1', 'RAILS_ENV' => 'production' }
      stdout, stderr, status = Open3.capture3(env, 'bash', Rails.root.join('docker-entrypoint.sh').to_s, 'export', unsetenv_others: true)
      assert status.success?, stderr
      calls = stdout.lines.map(&:strip)
      mkdir = calls.index { |line| line.start_with?('mkdir:-p ') && line.split.include?('/export') }
      chown = calls.index { |line| line.start_with?('chown:') && line.split.include?('deltabadger:deltabadger') && line.split.include?('/export') }
      gosu = calls.index { |line| line.start_with?('gosu:deltabadger ') }
      assert mkdir, 'root must create /export'
      assert chown, 'root must give the app user ownership of /export'
      assert gosu, 'entrypoint must drop privileges'
      assert_operator mkdir, :<, chown
      assert_operator chown, :<, gosu
    end
  end

  test 'export loads existing secrets with environment precedence and only runs the portable task' do
    Dir.mktmpdir do |dir|
      secrets = File.join(dir, '.secrets')
      original = "SECRET_KEY_BASE=stale\nACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=stored-primary\n" \
                 "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT=stored-salt\nSMTP_PASSWORD=not-exported\n"
      File.write(secrets, original)
      File.write(File.join(dir, 'id'), "#!/bin/sh\necho 1000\n")
      File.write(File.join(dir, 'bundle'), <<~SH)
        #!/bin/bash
        if [ "$2" = ruby ]; then exit 0; fi
        printf '%s\n' "COMMAND=$*" "ENTRYPOINT=$EXPORT_ENTRYPOINT" "SECRET=$SECRET_KEY_BASE" \
          "PRIMARY=$ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY" "SALT=$ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT" "SMTP=$SMTP_PASSWORD"
      SH
      FileUtils.chmod(0o755, [File.join(dir, 'id'), File.join(dir, 'bundle')])
      env = { 'PATH' => "#{dir}:#{ENV.fetch('PATH')}", 'SECRETS_FILE' => secrets, 'SECRET_KEY_BASE' => 'effective',
              'EXPORT_SCRATCH' => '1', 'RAILS_ENV' => 'production' }
      stdout, stderr, status = Open3.capture3(env, 'bash', Rails.root.join('docker-entrypoint.sh').to_s, 'export', unsetenv_others: true)
      assert status.success?, stderr
      ['COMMAND=exec rails export:portable', 'ENTRYPOINT=1', 'SECRET=effective', 'PRIMARY=stored-primary', 'SALT=stored-salt', 'SMTP='].each do |line|
        assert_includes stdout.lines.map(&:strip), line
      end
      assert_equal original, File.read(secrets)
      File.delete(secrets)
      stdout, stderr, status = Open3.capture3(env, 'bash', Rails.root.join('docker-entrypoint.sh').to_s, 'export', unsetenv_others: true)
      assert status.success?, stderr
      assert_includes stdout, 'SECRET=effective'
      refute File.exist?(secrets), 'export must not generate .secrets'
    end
  end
end
