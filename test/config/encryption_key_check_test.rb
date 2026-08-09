# test/config/encryption_key_check_test.rb
require 'test_helper'

class EncryptionKeyCheckTest < ActiveSupport::TestCase
  def stored_data_readable!(value)
    EncryptedCredentials::Report.any_instance
                                .stubs(:data_written_under_another_key?).returns(!value)
  end

  # capture_io on every emit_warning! call: the banner goes to stderr whatever the logger is,
  # so an uncaptured call prints it into the test output. Capturing also lets these assert the
  # thing that matters — that the operator sees it — rather than only what the caller gets back.
  test 'says nothing when everything stored decrypts' do
    stored_data_readable!(true)

    assert_equal :readable, EncryptionKeyCheck.readability(logger: nil)
    _out, err = capture_io { assert_nil EncryptionKeyCheck.emit_warning!(logger: nil) }

    assert_empty err
  end

  test 'warns when something stored cannot be read' do
    stored_data_readable!(false)
    assert_equal :unreadable, EncryptionKeyCheck.readability(logger: nil)

    emitted = nil
    _out, err = capture_io { emitted = EncryptionKeyCheck.emit_warning!(logger: nil) }

    assert_equal EncryptionKeyCheck::WARNING, emitted
    assert_equal EncryptionKeyCheck::WARNING, err
  end

  # A database that is not ready is the normal state during db:prepare, not a fault. It must
  # not raise, and it must not print the banner.
  test 'an unprepared database is indeterminate and produces no banner' do
    EncryptedCredentials::Report.any_instance
                                .stubs(:data_written_under_another_key?)
                                .raises(ActiveRecord::StatementInvalid, 'no such table: rules')

    assert_equal :indeterminate, EncryptionKeyCheck.readability(logger: nil)
    _out, err = capture_io do
      assert_nothing_raised { assert_nil EncryptionKeyCheck.emit_warning!(logger: nil) }
    end

    assert_empty err
  end

  # ...but it is not reported as health either.
  test 'an indeterminate check is logged rather than passed over' do
    EncryptedCredentials::Report.any_instance
                                .stubs(:data_written_under_another_key?).raises(NoMethodError, 'boom')
    logger = mock('logger')
    logger.expects(:info).with(regexp_matches(/did not complete/))

    assert_equal :indeterminate, EncryptionKeyCheck.readability(logger: logger)
  end

  # Nothing here may take an instance down. An instance in this state still holds recoverable
  # data and is the one that needs the recovery task; refusing to run would be the lockout
  # this whole change exists to prevent.
  test 'no state raises' do
    [true, false].each do |readable|
      stored_data_readable!(readable)

      capture_io { assert_nothing_raised { EncryptionKeyCheck.emit_warning!(logger: nil) } }
    end
  end

  test 'the warning names the encryption key variables' do
    assert_includes EncryptionKeyCheck::WARNING, EncryptionKeys::PRIMARY_KEY_VAR
    assert_includes EncryptionKeyCheck::WARNING, EncryptionKeys::SALT_VAR
  end

  # It must not tell anyone to re-enter credentials: that would overwrite ciphertext a
  # corrected key could still recover.
  test 'the warning says to restore the previous values, not to re-enter data' do
    assert_match(/restore/i, EncryptionKeyCheck::WARNING)
    assert_no_match(/re-enter/i, EncryptionKeyCheck::WARNING)
  end

  test 'the warning names the command that shows what is stored' do
    assert_includes EncryptionKeyCheck::WARNING, 'deltabadger:encryption:report'
  end

  # The module can be perfect and never called. Locating the boot branch by source text is the
  # technique test/config/secret_key_base_guard_test.rb already uses for the same reason.
  test 'the check is wired into boot, in production, on the deferred hook' do
    source = Rails.root.join('config/initializers/encryption_key_check.rb').read

    assert_match(/after_initialize/, source)
    assert_match(/Rails\.env\.production\?/, source)
    assert_match(/EncryptionKeyCheck\.emit_warning!/, source)
  end
end
