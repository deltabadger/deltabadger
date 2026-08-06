require 'test_helper'
require_relative '../../support/encrypted_credentials_test_helpers'

module EncryptedCredentials
  class ReportTest < ActiveSupport::TestCase
    include EncryptedCredentialsTestHelpers

    setup do
      @user = create(:user)
      @exchange = create(:binance_exchange)
      @asset = create(:asset)
    end

    def withdrawal_rule(address: 'wallet-one')
      Rules::Withdrawal.create!(
        user: @user, exchange: @exchange, asset: @asset, address: address,
        threshold_type: 'fee_percentage', max_fee_percentage: '1.0', status: :stopped
      )
    end

    test 'identifies each api key well enough to revoke it at the exchange' do
      create(:api_key, user: @user, exchange: @exchange, key_type: :trading)

      row = Report.new.call.api_keys.sole

      assert_equal @user.email, row[:user]
      assert_equal @exchange.name, row[:exchange]
      assert_equal 'trading', row[:key_type]
    end

    test 'lists the settings that hold credentials' do
      AppConfig.set(AppConfig::COINGECKO_API_KEY, 'cg-key')

      assert_includes Report.new.call.app_config_keys, AppConfig::COINGECKO_API_KEY
    end

    # Nothing enforces one fee key per exchange: the index is non-unique and the model has
    # no uniqueness validation. Deduplicating by exchange name would hide a live credential
    # from the operator's revocation list.
    test 'lists every fee api key, including duplicates for one exchange' do
      2.times { |n| FeeApiKey.create!(exchange: @exchange, key: "fee-#{n}", secret: "sec-#{n}") }

      assert_equal [@exchange.name, @exchange.name], Report.new.call.fee_api_keys
    end

    test 'shows withdrawal addresses in full so they can be copied' do
      withdrawal_rule

      assert_equal(['wallet-one'], Report.new.call.withdrawal_addresses.map { |row| row[:address] })
    end

    test 'reports unreadable values instead of printing ciphertext' do
      rule = withdrawal_rule
      blob = ciphertext_under_foreign_key('wallet-one')
      write_raw(Rule, rule.id, 'address', blob)

      assert_raises(ActiveRecord::Encryption::Errors::Decryption) do
        ActiveRecord::Encryption.encryptor.decrypt(blob)
      end
      assert_equal([Report::UNREADABLE], Report.new.call.withdrawal_addresses.map { |row| row[:address] })
    end

    test 'lists users with two-factor enabled' do
      @user.update!(otp_module: :enabled, otp_secret_key: 'SECRET123')

      assert_equal [@user.email], Report.new.call.two_factor_users
    end

    # Reset clears any non-null seed regardless of the module flag, so the report has to
    # select the same rows or it under-reports what is about to be destroyed.
    test 'lists users whose two-factor is off but who still hold a seed' do
      @user.update!(otp_module: :disabled, otp_secret_key: 'SECRET123')

      assert_equal [@user.email], Report.new.call.two_factor_users
    end

    # Report and Reset share one scope precisely so they can't silently drift apart. This
    # doesn't just re-check that scope in isolation — it runs the real Reset against the
    # report's own output, so a change to either side that stops matching the other fails
    # here even though each service's other tests still pass on their own.
    #
    # has_one_time_password's before_create hook auto-generates an otp_secret_key for every
    # user at signup, active or not, so a truly bare account is not the state a real signup
    # starts in — it's constructed here with update_column (bypassing that hook) purely to
    # exercise the scope's boundary: disabled-with-a-seed must be reported, disabled-with-no-
    # seed must not.
    test 'lists exactly the users whose two-factor material Reset actually clears' do
      @user.update_column(:otp_secret_key, nil)
      cleared = create(:user, otp_module: :disabled, otp_secret_key: 'SECRET123')
      untouched = create(:user, otp_module: :disabled)
      untouched.update_column(:otp_secret_key, nil)

      reported = Report.new.call.two_factor_users
      Reset.new.call

      assert_equal [cleared.email], reported
      assert_nil cleared.reload.read_attribute_before_type_cast(:otp_secret_key)
      assert_predicate cleared, :otp_module_disabled?
      assert_nil untouched.reload.read_attribute_before_type_cast(:otp_secret_key)
    end

    # The predicate that answers "was this database written under a key other than the one
    # we are running now" — the only evidence available on an install whose operator has
    # already rotated their secret. It reports what it observes and nothing more; deciding
    # that a rotation means the old key was published is the caller's job.
    test 'does not claim another key was used when everything decrypts under the current one' do
      create(:api_key, user: @user, exchange: @exchange)
      @user.update!(otp_module: :enabled, otp_secret_key: 'SECRET123')
      withdrawal_rule

      assert_not Report.new.data_written_under_another_key?
    end

    test 'detects data written under another key' do
      rule = withdrawal_rule
      write_raw(Rule, rule.id, 'address', ciphertext_under_foreign_key('wallet-one'))

      assert Report.new.data_written_under_another_key?
    end

    # Sampling one column would miss this operator entirely: withdrawal rules are optional
    # and most installs have none, so the check has to look across every covered attribute
    # rather than the first one it thinks of.
    test 'detects data written under another key from an attribute other than the address' do
      key = create(:api_key, user: @user, exchange: @exchange)
      write_raw(ApiKey, key.id, 'secret', ciphertext_under_foreign_key('sk-live-secret'))

      assert_equal 0, Rules::Withdrawal.count
      assert Report.new.data_written_under_another_key?
    end

    test 'changes nothing' do
      create(:api_key, user: @user, exchange: @exchange)
      AppConfig.set(AppConfig::COINGECKO_API_KEY, 'cg-key')

      assert_no_changes -> { [ApiKey.count, AppConfig.count, Rule.count, User.count] } do
        Report.new.call
      end
    end
  end
end
