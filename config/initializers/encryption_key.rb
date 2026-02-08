require "digest"

module EncryptionKey
  def self.derived_key
    @derived_key ||= Digest::SHA256.digest(ENV.fetch("APP_ENCRYPTION_KEY"))
  end
end
