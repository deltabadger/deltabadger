class FeeApiKey < ApplicationRecord
  belongs_to :exchange

  encrypts :key
  encrypts :secret
  encrypts :passphrase

  delegate :name, to: :exchange, prefix: true
end
