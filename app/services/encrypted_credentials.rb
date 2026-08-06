# Every encrypted attribute in the app, and how the reset disposes of it.
#
# Deliberately an explicit registry rather than something derived at runtime: a purely
# reflective check would silently do nothing for a model that happens to have no rows,
# and would give no signal when a new `encrypts` declaration appears. The parity test in
# test/services/encrypted_credentials/coverage_test.rb fails until this list and the
# declarations agree, which forces whoever adds the fourteenth attribute to decide how
# the reset should handle it.
#
# :delete_rows  — the row exists only to hold the credential; drop it entirely.
# :null_columns — the row carries other state worth keeping; clear the column only.
module EncryptedCredentials
  COVERAGE = {
    'ApiKey' => {
      strategy: :delete_rows,
      attributes: %i[key secret passphrase access_token rsa_signature_key rsa_encryption_key dh_param]
    },
    'FeeApiKey' => { strategy: :delete_rows, attributes: %i[key secret passphrase] },
    'AppConfig' => { strategy: :delete_rows, attributes: %i[value] },
    'User' => { strategy: :null_columns, attributes: %i[otp_secret_key] },
    'Rules::Withdrawal' => { strategy: :null_columns, attributes: %i[address] }
  }.freeze
end
