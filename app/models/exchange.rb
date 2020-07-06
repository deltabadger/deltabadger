class Exchange < ApplicationRecord
  KRAKEN_CURRENCIES = %w[USD USDT USDC EUR CHF GBP CAD].freeze
  BITBAY_CURRENCIES = %w[USD USDC EUR PLN].freeze
  BITCLUDE_CURRENCIES = %w[USD EUR PLN GBP].freeze
  DEFAULT_CURRENCIES = %w[USD EUR].freeze

  def currencies
    case name.downcase
    when 'kraken' then KRAKEN_CURRENCIES
    when 'bitbay' then BITBAY_CURRENCIES
    when 'bitclude' then BITCLUDE_CURRENCIES
    else DEFAULT_CURRENCIES
    end
  end
end
