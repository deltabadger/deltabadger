class FeeApiKey < ApplicationRecord
  belongs_to :exchange

  attr_encrypted :key, key: EncryptionKey.derived_key
  attr_encrypted :secret, key: EncryptionKey.derived_key
  attr_encrypted :passphrase, key: EncryptionKey.derived_key

  delegate :name, to: :exchange, prefix: true
end
