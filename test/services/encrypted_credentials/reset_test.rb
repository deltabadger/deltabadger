require 'test_helper'
require_relative '../../support/encrypted_credentials_test_helpers'

module EncryptedCredentials
  class ResetTest < ActiveSupport::TestCase
    include EncryptedCredentialsTestHelpers

    setup do
      @user = create(:user)
      @exchange = create(:binance_exchange)
      @asset = create(:asset)
      @api_key = create(:api_key, user: @user, exchange: @exchange)
    end

    def seed_every_encrypted_attribute
      @user.update!(otp_module: :enabled, otp_secret_key: 'SECRET123')
      @api_key.update!(
        passphrase: 'pp', access_token: 'at',
        rsa_signature_key: 'rsa-sig', rsa_encryption_key: 'rsa-enc', dh_param: 'dh'
      )
      FeeApiKey.create!(exchange: @exchange, key: 'fee-key', secret: 'fee-secret', passphrase: 'fee-pp')
      AppConfig.set(AppConfig::COINGECKO_API_KEY, 'cg-key')
      Rules::Withdrawal.create!(
        user: @user, exchange: @exchange, asset: @asset, address: 'wallet-one',
        threshold_type: 'fee_percentage', max_fee_percentage: '1.0', status: :scheduled
      )
      # Both branches of stop_bots' respond_to?(:cancel_scheduled_action_jobs) split:
      # Automation::Schedulable is included by the DCA subclasses but not by Bots::Signal.
      # Without a bot of each kind here, Bot.working is empty and stop_bots runs unexercised
      # in a test that is supposed to guard the whole service's core property. base/quote are
      # shared explicitly because each factory otherwise creates its own :bitcoin/:usd pair,
      # colliding on the asset's unique external_id.
      base_asset = create(:asset, :bitcoin)
      quote_asset = create(:asset, :usd)
      create(:dca_single_asset, user: @user, exchange: @exchange, base_asset:, quote_asset:, status: :scheduled)
      create(:signal_bot, user: @user, exchange: @exchange, base_asset:, quote_asset:, status: :scheduled)
    end

    # THE core property. Asserted at the encryption boundary rather than by observing the
    # outcome: with support_unencrypted_data on, an implementation that DOES decrypt still
    # produces a correct-looking result, so only instrumenting the encryptor can tell the
    # difference.
    test 'never decrypts anything' do
      seed_every_encrypted_attribute
      ActiveRecord::Encryption.encryptor.expects(:decrypt).never

      Reset.new.call
    end

    test 'clears everything even when no stored value can be decrypted' do
      seed_every_encrypted_attribute
      rule = Rule.find_by(type: 'Rules::Withdrawal')
      write_raw(ApiKey, @api_key.id, 'key', ciphertext_under_foreign_key('sk-live-key'))
      write_raw(User, @user.id, 'otp_secret_key', ciphertext_under_foreign_key('otp'))
      write_raw(Rule, rule.id, 'address', ciphertext_under_foreign_key('wallet-one'))

      assert_nothing_raised { Reset.new.call }

      assert_equal 0, ApiKey.count
      assert_nil @user.reload.read_attribute_before_type_cast(:otp_secret_key)
      assert_nil rule.reload.read_attribute_before_type_cast(:address)
    end

    # Guards the test above. Without it, that test could pass while the fixture was merely
    # double-encrypted rather than genuinely foreign — which is what happens if write_raw
    # is ever replaced by update_all.
    test 'the foreign ciphertext really is unreadable by this install' do
      blob = ciphertext_under_foreign_key('sk-live-secret')
      write_raw(ApiKey, @api_key.id, 'key', blob)

      assert_raises(ActiveRecord::Encryption::Errors::Decryption) do
        ActiveRecord::Encryption.encryptor.decrypt(blob)
      end
      assert_equal blob, @api_key.reload.key,
                   'a failed decrypt must pass the stored ciphertext straight through'
    end

    test 'deletes api keys but keeps their account transactions' do
      transaction = create(:account_transaction, exchange: @exchange, api_key: @api_key)

      Reset.new.call

      assert_equal 0, ApiKey.count
      assert AccountTransaction.exists?(transaction.id), 'transaction history must survive'
      assert_nil transaction.reload.api_key_id
    end

    test 'disables two-factor authentication' do
      @user.update!(otp_module: :enabled, otp_secret_key: 'SECRET123')

      summary = Reset.new.call

      @user.reload
      assert @user.otp_module_disabled?
      assert_nil @user.read_attribute_before_type_cast(:otp_secret_key)
      assert_equal 1, summary.two_factor_disabled
    end

    # Expectation is on the concrete subclass, not Bot: cancel_scheduled_action_jobs comes
    # from Automation::Schedulable, which the subclass includes and Bot does not.
    test 'stops working bots and cancels their scheduled jobs' do
      bot = create(:dca_single_asset, user: @user, exchange: @exchange, status: :scheduled)
      Bots::DcaSingleAsset.any_instance.expects(:cancel_scheduled_action_jobs).at_least_once

      summary = Reset.new.call

      assert bot.reload.stopped?
      assert_not_nil bot.stopped_at
      assert_equal 1, summary.bots_stopped
    end

    # Bots::Signal does NOT include Automation::Schedulable, so calling
    # cancel_scheduled_action_jobs on every working bot raises NoMethodError and aborts the
    # recovery — on an install that has one, which is the worst possible time to find out.
    test 'stops a working bot that has no schedulable jobs' do
      bot = create(:signal_bot, user: @user, exchange: @exchange, status: :scheduled)

      summary = Reset.new.call

      assert bot.reload.stopped?
      assert_equal 1, summary.bots_stopped
    end

    test 'stops withdrawal rules and clears their addresses' do
      rule = Rules::Withdrawal.create!(
        user: @user, exchange: @exchange, asset: @asset, address: 'wallet-one',
        threshold_type: 'fee_percentage', max_fee_percentage: '1.0', status: :scheduled
      )

      summary = Reset.new.call

      rule.reload
      assert rule.stopped?
      assert_nil rule.read_attribute_before_type_cast(:address)
      assert_equal 1, summary.rules_stopped
      assert_equal 1, summary.withdrawal_addresses_cleared
    end

    test 'deletes fee api keys and app configs' do
      FeeApiKey.create!(exchange: @exchange, key: 'fee-key', secret: 'fee-secret')
      AppConfig.set(AppConfig::COINGECKO_API_KEY, 'cg-key')

      summary = Reset.new.call

      assert_equal 0, FeeApiKey.count
      assert_equal 0, AppConfig.count
      assert_equal 1, summary.fee_api_keys_deleted
      assert_equal 1, summary.app_configs_deleted
    end

    test 'reports whether irreplaceable IBKR credentials were destroyed' do
      ibkr = create(:ibkr_exchange)
      create(:api_key, user: @user, exchange: ibkr, raw_key: 'ibkr-consumer')

      assert Reset.new.call.ibkr_credentials_destroyed
    end

    test 'does not claim IBKR credentials were destroyed when there were none' do
      assert_not Reset.new.call.ibkr_credentials_destroyed
    end
  end
end
