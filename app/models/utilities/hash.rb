module Utilities
  module Hash
    def self.dig_or_raise(hash, *keys)
      value = hash.dig(*keys)
      raise KeyError, "Key path #{keys.join(' -> ')} not found" if value.nil?

      value
    end

    def self.safe_dig(hash, *keys)
      current = hash
      keys.each do |key|
        return nil unless current.respond_to?(:dig)

        current = current[key]
      end
      current
    end
  end
end
