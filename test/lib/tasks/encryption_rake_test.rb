require 'test_helper'
require 'rake'
require_relative '../../support/encrypted_credentials_test_helpers'

class EncryptionRakeTest < ActiveSupport::TestCase
  include EncryptedCredentialsTestHelpers

  # `load` rather than Rake.application.rake_require: rake_require records the file in
  # $LOADED_FEATURES, so with a fresh Rake::Application per test every run after the first
  # would silently get an application with no tasks defined, and the order would matter.
  setup do
    Rake.application = Rake::Application.new
    Rake::Task.define_task(:environment)
    load Rails.root.join('lib/tasks/encryption.rake')
    @user = create(:user)
    @exchange = create(:binance_exchange)
  end

  teardown do
    Rake.application = nil
    ENV.delete('CONFIRM')
    ENV.delete('CONFIRM_IBKR')
    ENV.delete('PUBLISHED')
  end

  # Puts a credential in the database that this install's key cannot read — the state of an
  # operator who already replaced their SECRET_KEY_BASE. published? is stubbed false because
  # that is the truth for them: the value they are running now is strong. The old one, which
  # actually encrypted this row, may well have been the published placeholder.
  def stored_under_another_key!
    key = create(:api_key, user: @user, exchange: @exchange)
    write_raw(ApiKey, key.id, 'key', ciphertext_under_foreign_key('sk-live-key'))
    SecretKeyBaseGuard.stubs(:published?).returns(false)
  end

  test 'reset refuses without confirmation and changes nothing' do
    create(:api_key, user: @user, exchange: @exchange)

    assert_raises(SystemExit) { capture_io { Rake::Task['deltabadger:encryption:reset'].invoke } }

    assert_equal 1, ApiKey.count, 'an unconfirmed reset must not touch anything'
  end

  test 'reset runs with confirmation' do
    create(:api_key, user: @user, exchange: @exchange)
    ENV['CONFIRM'] = 'clear-credentials'

    capture_io { Rake::Task['deltabadger:encryption:reset'].invoke }

    assert_equal 0, ApiKey.count
  end

  # Re-registering IBKR's RSA material means going through their portal and waiting days
  # for activation, so it must not go the same way as a two-minute exchange key.
  test 'reset refuses to destroy IBKR credentials without the extra confirmation' do
    create(:api_key, user: @user, exchange: create(:ibkr_exchange), raw_key: 'ibkr-consumer')
    ENV['CONFIRM'] = 'clear-credentials'

    assert_raises(SystemExit) { capture_io { Rake::Task['deltabadger:encryption:reset'].invoke } }

    assert_equal 1, ApiKey.count
  end

  test 'reset destroys IBKR credentials with both confirmations' do
    create(:api_key, user: @user, exchange: create(:ibkr_exchange), raw_key: 'ibkr-consumer')
    ENV['CONFIRM'] = 'clear-credentials'
    ENV['CONFIRM_IBKR'] = 'yes'

    capture_io { Rake::Task['deltabadger:encryption:reset'].invoke }

    assert_equal 0, ApiKey.count
  end

  test 'report prints withdrawal addresses and changes nothing' do
    Rules::Withdrawal.create!(
      user: @user, exchange: @exchange, asset: create(:asset), address: 'wallet-one',
      threshold_type: 'fee_percentage', max_fee_percentage: '1.0', status: :stopped
    )

    out, = capture_io { Rake::Task['deltabadger:encryption:report'].invoke }

    assert_match 'wallet-one', out
    assert_equal 1, Rule.count
  end

  test 'report tells the operator to revoke when the secret was published' do
    create(:api_key, user: @user, exchange: @exchange)
    SecretKeyBaseGuard.stubs(:published?).returns(true)

    out, = capture_io { Rake::Task['deltabadger:encryption:report'].invoke }

    assert_match(/revoke/i, out)
  end

  # This is the split from published-secret-recovery: a privately generated secret that is
  # merely short is not an exposure, and README.md tells that reader to skip revocation.
  # Before this the report unconditionally said "REVOKE" and "anyone holding a copy of this
  # database can still trade with them" — false for this case, and directly contradicting
  # the README it sits next to.
  test 'report does not demand revocation when the secret was never published' do
    create(:api_key, user: @user, exchange: @exchange)
    SecretKeyBaseGuard.stubs(:published?).returns(false)

    out, = capture_io { Rake::Task['deltabadger:encryption:report'].invoke }

    assert_no_match(/revoke/i, out)
    assert_no_match(/anyone/i, out)
  end

  test "reset's confirmation prompt demands revocation when the secret was published" do
    SecretKeyBaseGuard.stubs(:published?).returns(true)

    _out, err = capture_io { assert_raises(SystemExit) { Rake::Task['deltabadger:encryption:reset'].invoke } }

    assert_match(/revoke/i, err)
  end

  # Same split as above, applied to the pre-confirmation warning: before this fix it told
  # every reader to REVOKE every credential, twice, regardless of whether their secret was
  # ever published — walking a private-secret operator through the README's own exemption
  # and then contradicting it.
  test "reset's confirmation prompt does not demand revocation when the secret was never published" do
    SecretKeyBaseGuard.stubs(:published?).returns(false)

    _out, err = capture_io { assert_raises(SystemExit) { Rake::Task['deltabadger:encryption:reset'].invoke } }

    assert_no_match(/revoke/i, err)
  end

  # The operator this whole recovery most needs to reach is the one who already changed
  # their secret: the stored ciphertext is unreadable, two-factor sign-in is broken, and they
  # cannot get into the UI at all. Classifying from Rails.application.secret_key_base alone
  # reads their NEW strong value, finds it unpublished, and reassures the one person whose
  # database is most likely to be sitting on a published key.
  test 'report demands revocation when the stored data was written under another key' do
    stored_under_another_key!

    out, = capture_io { Rake::Task['deltabadger:encryption:report'].invoke }

    assert_match(/revoke/i, out)
  end

  test "reset's confirmation prompt demands revocation when the stored data was written under another key" do
    stored_under_another_key!

    _out, err = capture_io { assert_raises(SystemExit) { Rake::Task['deltabadger:encryption:reset'].invoke } }

    assert_match(/revoke/i, err)
  end

  # Also pins the ordering: the reset destroys the very evidence this classification reads,
  # so a check made after the clear would find a spotless database and print the reassuring
  # summary to the operator who most needs the other one.
  test 'reset demands revocation in its post-run summary when the stored data was written under another key' do
    stored_under_another_key!
    ENV['CONFIRM'] = 'clear-credentials'

    out, = capture_io { Rake::Task['deltabadger:encryption:reset'].invoke }

    assert_match(/revoke/i, out)
  end

  # The escape hatch for what neither check can see: an operator who knows the secret their
  # data was written under was published, but whose database happens to decrypt cleanly —
  # they are still running that key, or they re-encrypted before asking.
  test 'PUBLISHED=yes forces the compromised path' do
    create(:api_key, user: @user, exchange: @exchange)
    SecretKeyBaseGuard.stubs(:published?).returns(false)
    ENV['PUBLISHED'] = 'yes'

    out, = capture_io { Rake::Task['deltabadger:encryption:report'].invoke }

    assert_match(/revoke/i, out)
  end

  # An operator who cannot run the report cannot follow the procedure, so the abort has to
  # name the override itself rather than leaving it to the README.
  test "reset's confirmation prompt documents the PUBLISHED override" do
    _out, err = capture_io { assert_raises(SystemExit) { Rake::Task['deltabadger:encryption:reset'].invoke } }

    assert_match(/PUBLISHED=yes/, err)
  end

  # The REST and MCP tokens are a credential the operator cannot see being destroyed
  # anywhere else: they are not in the report's inventory, because they are not encrypted
  # attributes. The count is how they learn to reconnect their clients afterwards.
  test 'reset reports how many REST API and MCP tokens it deleted' do
    application = Doorkeeper::Application.create!(
      name: 'Test MCP Client', redirect_uri: 'https://localhost/callback',
      confidential: false, scopes: 'mcp'
    )
    Doorkeeper::AccessToken.create!(
      application: application, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'mcp', expires_in: nil
    )
    ENV['CONFIRM'] = 'clear-credentials'

    out, = capture_io { Rake::Task['deltabadger:encryption:reset'].invoke }

    assert_match(/REST API and MCP tokens:\s+1/, out)
  end

  # Same defect as the PUBLISHED override, in the abort the operator hits first. Every documented
  # invocation is a `docker compose run`, so CONFIRM_IBKR set in the host shell never reaches the
  # one-shot container and the task aborts again on exactly the same message.
  test 'the IBKR confirmation is shown in a form the documented commands can pass' do
    create(:api_key, user: @user, exchange: create(:ibkr_exchange), raw_key: 'ibkr-consumer')
    ENV['CONFIRM'] = 'clear-credentials'

    _out, err = capture_io { assert_raises(SystemExit) { Rake::Task['deltabadger:encryption:reset'].invoke } }

    assert_match(/-e CONFIRM_IBKR=yes/, err)
  end

  # The last thing the operator reads after the destructive step, in a terminal, often with
  # no README open. It said "blank the SECRET_KEY_BASE value in .env.docker and start the
  # app again" — which permits `docker compose start`, reusing the stopped container's
  # baked-in environment, and says nothing about /app/storage/.secrets, which is where the
  # key actually lives on a default install. Following it verbatim rotates nothing.
  test 'reset tells the operator to recreate the container and rotate the stored key' do
    ENV['CONFIRM'] = 'clear-credentials'

    out, = capture_io { Rake::Task['deltabadger:encryption:reset'].invoke }

    assert_match(/docker compose up -d --force-recreate/, out)
    assert_match(%r{/app/storage/\.secrets}, out)
    # A compose-file install must not be told to blank and let the containers regenerate:
    # that file runs two services off one volume and each would generate its own key.
    assert_match(/openssl rand -hex 64/, out)
  end

  test 'reset does not demand revocation in its post-run summary when the secret was never published' do
    create(:api_key, user: @user, exchange: @exchange)
    ENV['CONFIRM'] = 'clear-credentials'
    SecretKeyBaseGuard.stubs(:published?).returns(false)

    out, = capture_io { Rake::Task['deltabadger:encryption:reset'].invoke }

    assert_no_match(/revoke/i, out)
  end

  test 'derived_keys prints both variables with the values this install derives' do
    output = capture_io { Rake::Task['deltabadger:encryption:derived_keys'].invoke }.first
    secret = Rails.application.secret_key_base

    assert_includes output, "#{EncryptionKeys::PRIMARY_KEY_VAR}=#{EncryptionKeys.derived_primary_key(secret)}"
    assert_includes output, "#{EncryptionKeys::SALT_VAR}=#{EncryptionKeys.derived_salt(secret)}"
  end

  # The values it prints have to be the ones the initializer resolves, or setting them would
  # change the key and make everything stored unreadable.
  test 'derived_keys matches what the initializer resolves with nothing configured' do
    output = capture_io { Rake::Task['deltabadger:encryption:derived_keys'].invoke }.first
    secret = Rails.application.secret_key_base

    assert_includes output, EncryptionKeys.primary_key({}, secret: secret)
    assert_includes output, EncryptionKeys.key_derivation_salt({}, secret: secret)
  end

  # On an install that already has its own keys, the derived values are NOT the ones
  # encrypting its data. Printing them next to an instruction to copy them in would replace
  # working keys with wrong ones.
  test 'derived_keys prints no values once the install has its own keys' do
    EncryptionKeys.stubs(:independent?).returns(true)
    output = capture_io { Rake::Task['deltabadger:encryption:derived_keys'].invoke }.first
    secret = Rails.application.secret_key_base

    assert_not_includes output, EncryptionKeys.derived_primary_key(secret)
    assert_not_includes output, EncryptionKeys.derived_salt(secret)
    assert_match(/already/i, output)
  end

  # The common failure is SECRET_KEY_BASE replaced on an install with no configured pair.
  # independent? is false there, so without this guard the task would print keys derived from
  # the NEW, wrong secret and describe them as the values already in use.
  test 'derived_keys prints no values when stored data cannot be read' do
    EncryptionKeyCheck.stubs(:readability).returns(:unreadable)
    output = capture_io { Rake::Task['deltabadger:encryption:derived_keys'].invoke }.first

    assert_not_includes output, EncryptionKeys.derived_primary_key(Rails.application.secret_key_base)
    assert_match(/not printed/i, output)
  end

  # Same for a check that could not complete. A locked or damaged database says nothing about
  # which keys are right, and printing a pair on that basis is what makes a loss permanent.
  test 'derived_keys prints no values when readability could not be determined' do
    EncryptionKeyCheck.stubs(:readability).returns(:indeterminate)
    output = capture_io { Rake::Task['deltabadger:encryption:derived_keys'].invoke }.first

    assert_not_includes output, EncryptionKeys.derived_primary_key(Rails.application.secret_key_base)
    assert_match(/not printed/i, output)
  end

  # The entrypoint owns the secrets file. A task that wrote it would be a second writer with
  # no coordination, and on a compose-supplied install would write to a file that governs
  # nothing.
  test 'derived_keys writes nothing' do
    File.expects(:write).never
    File.expects(:open).never

    capture_io { Rake::Task['deltabadger:encryption:derived_keys'].invoke }
  end

  # The reset output is the authoritative copy of this procedure — an operator in a terminal
  # is not reading the README. Left set, these re-encrypt the replacement credentials under
  # the key the reset exists to get away from.
  test 'reset tells an install with its own encryption keys to clear them' do
    EncryptionKeys.stubs(:independent?).returns(true)
    ENV['CONFIRM'] = 'clear-credentials'
    output = capture_io { Rake::Task['deltabadger:encryption:reset'].invoke }.first

    assert_includes output, EncryptionKeys::PRIMARY_KEY_VAR
    assert_includes output, EncryptionKeys::SALT_VAR
  end

  # An install with a database is never handed a generated pair, so promising one would leave
  # the operator re-coupled to SECRET_KEY_BASE without knowing it.
  test 'reset does not promise a pair that will not be generated' do
    EncryptionKeys.stubs(:independent?).returns(true)
    ENV['CONFIRM'] = 'clear-credentials'
    output = capture_io { Rake::Task['deltabadger:encryption:reset'].invoke }.first

    assert_no_match(/will be generated|generated for you/i, output)
    assert_includes output, 'deltabadger:encryption:derived_keys'
  end

  test 'reset says nothing about them on an install that does not set them' do
    EncryptionKeys.stubs(:independent?).returns(false)
    ENV['CONFIRM'] = 'clear-credentials'
    output = capture_io { Rake::Task['deltabadger:encryption:reset'].invoke }.first

    assert_not_includes output, EncryptionKeys::PRIMARY_KEY_VAR
  end
end
