module Utilities
  module Hash
    def self.dig_or_raise(hash, *keys)
      value = hash.dig(*keys)
      raise KeyError, "Key path #{keys.join(' -> ')} not found" if value.nil?

      value
    end
  end
end
