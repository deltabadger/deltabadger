class Exchange < ApplicationRecord
  BINANCE_CURRENCIES =
    %w[USDT USDC USDS BUSD TUSD EUR GBP AUD PAX TRY BKRW IDRT NGN RUB ZAR UAH].freeze
  BITBAY_CURRENCIES = %w[USD USDC EUR PLN].freeze
  KRAKEN_CURRENCIES = %w[USD USDT USDC AUD DAI EUR CHF GBP CAD].freeze
  DEFAULT_CURRENCIES = %w[USD EUR].freeze

  def currencies
    case name.downcase
    when 'binance' then BINANCE_CURRENCIES
    when 'bitbay' then BITBAY_CURRENCIES
    when 'kraken' then KRAKEN_CURRENCIES
    else DEFAULT_CURRENCIES
    end
  end
end
