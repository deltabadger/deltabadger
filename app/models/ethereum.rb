class Ethereum
  def self.address_pattern
    '^0x[a-fA-F0-9]{40}$'
  end

  def self.valid_address?(address)
    address.match?(address_pattern)
  end
end
