class Bitcoin
  def self.address_pattern
    # It covers all four possible BTC address formats (from modern to legacy):
    # Taproot address - P2TR
    # Begins with bc1p, length from 42 to 62, case insensitive
    # SegWit address - P2WPKH
    # Begins with bc1q, length from 42 to 62, case insensitive
    # Script address - P2SH
    # Begins with 3, length from 26 to 35 (cannot have small el l , capital eye I, capital ou O and zero 0), case sensitive
    # Legacy address - P2PKH
    # Begins with 1, length from 26 to 35 (cannot have small el l , capital eye I, capital ou O and zero 0), case sensitive
    '^[13][a-km-zA-HJ-NP-Z1-9]{25,34}$|^[bB][cC]1[pPqQ][a-zA-Z0-9]{38,58}$'
  end

  def self.valid_address?(address)
    address.match?(address_pattern)
  end
end
