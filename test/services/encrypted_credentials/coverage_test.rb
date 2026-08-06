require 'test_helper'

module EncryptedCredentials
  class CoverageTest < ActiveSupport::TestCase
    # Structural, not data-driven. A new model with no rows, or a new attribute left NULL,
    # sails through a "read every row and assert nil" loop. This fails the moment an
    # `encrypts` declaration exists that the reset registry does not account for.
    test 'the coverage registry matches every encrypts declaration in the app' do
      assert_equal(declared_encrypted_attributes,
                   COVERAGE.transform_values { |entry| entry[:attributes].map(&:to_s).sort })
    end

    test 'no encrypted value survives the reset' do
      user = create(:user, otp_module: :enabled, otp_secret_key: 'SECRET123')
      exchange = create(:binance_exchange)
      create(:api_key, user: user, exchange: exchange, raw_key: 'k', raw_secret: 's', raw_passphrase: 'p')
        .update!(access_token: 'at', rsa_signature_key: 'sig', rsa_encryption_key: 'enc', dh_param: 'dh')
      FeeApiKey.create!(exchange: exchange, key: 'fk', secret: 'fs', passphrase: 'fp')
      AppConfig.set(AppConfig::COINGECKO_API_KEY, 'cg')
      Rules::Withdrawal.create!(
        user: user, exchange: exchange, asset: create(:asset), address: 'wallet-one',
        threshold_type: 'fee_percentage', max_fee_percentage: '1.0', status: :stopped
      )

      # Every attribute in the registry must actually hold a value first, or the assertion
      # after the reset proves nothing.
      COVERAGE.each do |model_name, entry|
        model = model_name.constantize
        entry[:attributes].each do |attribute|
          assert model.where.not(attribute => nil).exists?,
                 "#{model_name}##{attribute} was not seeded — the post-reset assertion would be vacuous"
        end
      end

      Reset.new.call

      COVERAGE.each do |model_name, entry|
        model = model_name.constantize
        entry[:attributes].each do |attribute|
          assert_not model.where.not(attribute => nil).exists?,
                     "#{model_name}##{attribute} survived the reset"
        end
      end
    end

    private

    def declared_encrypted_attributes
      Rails.application.eager_load!
      ApplicationRecord.descendants
                       .reject { |model| model.abstract_class? || model.encrypted_attributes.blank? }
                       .to_h { |model| [model.name, model.encrypted_attributes.map(&:to_s).sort] }
    end
  end
end
