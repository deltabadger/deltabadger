# Read-only inventory of everything EncryptedCredentials::Reset will destroy.
#
# Its real purpose is revocation, not record-keeping: deleting these rows does nothing to
# an exchange key an attacker already copied out of the database, so the operator needs
# enough detail — which user, which exchange, which key type — to go and revoke every one
# of them upstream.
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

    private

    def api_keys
      ApiKey.includes(:exchange, :user).map do |key|
        { user: key.user&.email, exchange: key.exchange&.name, key_type: key.key_type }
      end.sort_by { |row| [row[:exchange].to_s, row[:key_type].to_s] }
    end

    # Matches Reset#disable_two_factor, which clears any non-null seed regardless of the
    # module flag. Reporting only `otp_module: enabled` would under-report the destruction.
    def two_factor_users
      User.where.not(otp_secret_key: nil)
          .or(User.where.not(otp_module: User.otp_modules[:disabled]))
          .pluck(:email).sort
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
