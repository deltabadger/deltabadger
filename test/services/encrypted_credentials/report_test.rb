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
    # match that scope or it under-reports what is about to be destroyed.
    test 'lists users whose two-factor is off but who still hold a seed' do
      @user.update!(otp_module: :disabled, otp_secret_key: 'SECRET123')

      assert_equal [@user.email], Report.new.call.two_factor_users
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
