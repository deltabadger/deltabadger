class Fiat
  # The two `assets.category` values that mean cash. 'Currency' is what `currencies` below creates;
  # 'Fiat' is the local `usd` row the Alpaca sync keeps to its own convention.
  CATEGORIES = %w[Fiat Currency].freeze

  # A currency's logo is its flag, and the files ship in this repo (app/assets/images/flags) rather
  # than being fetched: a self-hosted install reads its market data from CoinGecko, which has no
  # image for a currency, so anything fetched would leave those installs blank forever. The hosted
  # market-data API does serve one now, and the flag deliberately wins over it — one currency, one
  # picture, whichever way the install gets its data.
  #
  # flag-icons' square (1x1) SVGs, the same set the language selector uses. EUR takes the EU flag.
  # A currency added to `currencies` needs its flag here too, or it renders logo-less; the test in
  # test/models/fiat_test.rb fails until it does.
  FLAGS = {
    'USD' => 'us', 'EUR' => 'eu', 'GBP' => 'gb', 'JPY' => 'jp', 'CHF' => 'ch',
    'CAD' => 'ca', 'AUD' => 'au', 'PLN' => 'pl', 'ARS' => 'ar', 'BRL' => 'br',
    'TRY' => 'tr', 'MXN' => 'mx', 'ZAR' => 'za', 'UAH' => 'ua', 'CZK' => 'cz',
    'RUB' => 'ru', 'IDR' => 'id'
  }.freeze

  def self.flag(symbol)
    FLAGS[symbol.to_s.upcase]
  end

  def self.currencies
    [
      {
        external_id: 'USD.FOREX', # eodhd ID
        symbol: 'USD',
        name: 'US Dollar',
        category: 'Currency',
        color: '#355E3B'
      },
      {
        external_id: 'EUR.FOREX', # eodhd ID
        symbol: 'EUR',
        name: 'Euro',
        category: 'Currency',
        color: '#003087'
      },
      {
        external_id: 'GBP.FOREX', # eodhd ID
        symbol: 'GBP',
        name: 'British Pound',
        category: 'Currency',
        color: '#4B0082'
      },
      {
        external_id: 'JPY.FOREX', # eodhd ID
        symbol: 'JPY',
        name: 'Japanese Yen',
        category: 'Currency',
        color: '#C1A36F'
      },
      {
        external_id: 'CHF.FOREX', # eodhd ID
        symbol: 'CHF',
        name: 'Swiss Franc',
        category: 'Currency',
        color: '#D52B1E'
      },
      {
        external_id: 'CAD.FOREX', # eodhd ID
        symbol: 'CAD',
        name: 'Canadian Dollar',
        category: 'Currency',
        color: '#D80621'
      },
      {
        external_id: 'AUD.FOREX', # eodhd ID
        symbol: 'AUD',
        name: 'Australian Dollar',
        category: 'Currency',
        color: '#3A9C9F'
      },
      {
        external_id: 'PLN.FOREX', # eodhd ID
        symbol: 'PLN',
        name: 'Polish Zloty',
        category: 'Currency',
        color: '#C0A98E'
      },
      {
        external_id: 'ARS.FOREX', # eodhd ID
        symbol: 'ARS',
        name: 'Argentine Peso',
        category: 'Currency',
        color: '#75AADB'
      },
      {
        external_id: 'BRL.FOREX', # eodhd ID
        symbol: 'BRL',
        name: 'Brazilian Real',
        category: 'Currency',
        color: '#009C3B'
      },
      {
        external_id: 'TRY.FOREX', # eodhd ID
        symbol: 'TRY',
        name: 'Turkish Lira',
        category: 'Currency',
        color: '#D45D5D'
      },
      {
        external_id: 'MXN.FOREX', # eodhd ID
        symbol: 'MXN',
        name: 'Mexican Peso',
        category: 'Currency',
        color: '#6C9A74'
      },
      {
        external_id: 'ZAR.FOREX', # eodhd ID
        symbol: 'ZAR',
        name: 'South African Rand',
        category: 'Currency',
        color: '#F9A825'
      },
      {
        external_id: 'UAHUSD.FOREX', # eodhd ID
        symbol: 'UAH',
        name: 'Ukrainian Hryvnia',
        category: 'Currency',
        color: '#B4A7D6'
      },
      {
        external_id: 'CZK.FOREX', # eodhd ID
        symbol: 'CZK',
        name: 'Czech Koruna',
        category: 'Currency',
        color: '#D36C6C'
      },
      {
        external_id: 'RUB.FOREX', # eodhd ID
        symbol: 'RUB',
        name: 'Russian Ruble',
        category: 'Currency',
        color: '#9C9CDE'
      },
      {
        external_id: 'IDR.FOREX', # eodhd ID
        symbol: 'IDR',
        name: 'Indonesian Rupiah',
        category: 'Currency',
        color: '#E74C3C'
      }
    ]
  end
end
