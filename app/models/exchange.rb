class Exchange < ApplicationRecord
  KRAKEN_CURRENCIES = %w[USD USDT USDC AUD DAI EUR CHF GBP CAD].freeze
  BITBAY_CURRENCIES = %w[USD USDC EUR PLN].freeze
  DEFAULT_CURRENCIES = %w[USD EUR].freeze

  def currencies
    case name.downcase
    when 'kraken' then KRAKEN_CURRENCIES
    when 'bitbay' then BITBAY_CURRENCIES
    else DEFAULT_CURRENCIES
    end
  end
end
