# Read-only inventory of everything EncryptedCredentials::Reset will destroy.
#
# Its real purpose is revocation, not record-keeping: deleting these rows does nothing to
# an exchange key an attacker already copied out of the database, so the operator needs
# enough detail — which user, which exchange, which key type — to go and revoke every one
# of them upstream.
#
# Unlike Reset, this deliberately DOES decrypt — that is the point, since the operator
# needs to read the withdrawal addresses back out. It is safe to do so here: readable
# guards every decrypt behind ActiveRecord::Encryption.encryptor.encrypted?, so a value
# this install's key cannot decrypt prints as <unreadable> instead of raising or leaking
# ciphertext.
#
# Prints withdrawal addresses in full; they are public destinations, and retyping one from
# memory is how funds go to the wrong place. Prints only the KEYS of app configs, never
# their values, which include SMTP and exchange credentials.
module EncryptedCredentials
  class Report
    UNREADABLE = '<unreadable>'.freeze

    Inventory = Struct.new(
      :api_keys, :fee_api_keys, :two_factor_users,
      :app_config_keys, :withdrawal_addresses,
      keyword_init: true
    )

    def call
      Inventory.new(
        api_keys: api_keys,
        # One entry per row, deliberately NOT deduplicated. Exchange has_one :fee_api_key,
        # but nothing enforces it — index_fee_api_keys_on_exchange_id is non-unique and the
        # model has no uniqueness validation — so a second credential for one exchange is
        # possible, and collapsing it would hide a secret that still needs rotating.
        fee_api_keys: FeeApiKey.includes(:exchange).map { |key| key.exchange&.name }.sort,
        two_factor_users: two_factor_users,
        app_config_keys: AppConfig.pluck(:key).sort,
        withdrawal_addresses: withdrawal_addresses
      )
    end

    # True when some stored encrypted value cannot be decrypted with the key this process is
    # running on, which proves that value was written under a DIFFERENT key.
    #
    # Named for what it observes, not for what a caller concludes. It does not know whether
    # the other key was published — only that the current secret says nothing about the data
    # sitting in this database, which is precisely the situation of an operator who has
    # already rotated and can no longer sign in.
    #
    # Looks across every attribute in COVERAGE rather than sampling one. A withdrawal
    # address, a passphrase, an access token: each is legitimately null on most installs, so
    # a single column would answer "no" for an operator whose API keys are unreadable.
    # Short-circuits on the first hit.
    def data_written_under_another_key?
      COVERAGE.any? do |model_name, entry|
        model = model_name.constantize
        entry[:attributes].any? { |attribute| unreadable_values?(model, attribute) }
      end
    end

    private

    def unreadable_values?(model, attribute)
      model.where.not(attribute => nil)
           .find_each
           .any? { |record| readable(record.public_send(attribute)) == UNREADABLE }
    end

    def api_keys
      ApiKey.includes(:exchange, :user).map do |key|
        { user: key.user&.email, exchange: key.exchange&.name, key_type: key.key_type }
      end.sort_by { |row| [row[:exchange].to_s, row[:key_type].to_s] }
    end

    # User.with_two_factor_material is shared with Reset#disable_two_factor so the two can't
    # drift onto different definitions of what counts as two-factor material: this report
    # exists to enumerate exactly what the reset clears.
    def two_factor_users
      User.with_two_factor_material.pluck(:email).sort
    end

    def withdrawal_addresses
      Rule.where(type: 'Rules::Withdrawal').includes(:exchange, :asset).map do |rule|
        { exchange: rule.exchange&.name, asset: rule.asset&.symbol, address: readable(rule.address) }
      end
    end

    # A value this install's key cannot decrypt comes back as raw ciphertext rather than
    # raising, because support_unencrypted_data is on. Detect that and say so, rather than
    # printing a base64 blob and letting the operator mistake it for their address.
    def readable(value)
      return nil if value.nil?

      ActiveRecord::Encryption.encryptor.encrypted?(value) ? UNREADABLE : value
    end
  end
end
