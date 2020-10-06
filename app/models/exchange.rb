class Exchange < ApplicationRecord
  BINANCE_BASES = %w[BTC]
  BINANCE_QUOTES =
    %w[USDT USDC USDS BUSD TUSD EUR GBP AUD PAX TRY BKRW IDRT NGN RUB ZAR UAH].freeze

  BITBAY_BASES = %w[BTC]
  BITBAY_QUOTES = %w[USD USDC EUR PLN].freeze

  KRAKEN_QUOTES = %w[USD USDT USDC AUD DAI EUR CHF GBP CAD].freeze
  KRAKEN_BASES = %w[XBT]

  DEFAULT_BASES = %w[XBT]
  DEFAULT_QUOTES = %w[USD EUR].freeze

  def bases
    case name.downcase
    when 'binance' then BINANCE_BASES
    when 'bitbay' then BITBAY_BASES
    when 'kraken' then KRAKEN_BASES
    else DEFAULT_BASES
    end
  end

  def quotes
    case name.downcase
    when 'binance' then BINANCE_QUOTES
    when 'bitbay' then BITBAY_QUOTES
    when 'kraken' then KRAKEN_QUOTES
    else DEFAULT_QUOTES
    end
  end
end
