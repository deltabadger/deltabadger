desc 'rake task to create delisted assets and tickers'
task create_delisted_assets_and_tickers: :environment do
  new_ticker(
    'cardano',
    'GBP.FOREX',
    'Exchanges::Binance',
    'ADAGBP', 'ADA', 'GBP', 8, 8, 2
  )

  new_ticker(
    'audius',
    'binance-peg-busd',
    'Exchanges::Binance',
    'AUDIOBUSD', 'AUDIO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'axie-infinity',
    'binance-peg-busd',
    'Exchanges::Binance',
    'AXSBUSD', 'AXS', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'binancecoin',
    'GBP.FOREX',
    'Exchanges::Binance',
    'BNBGBP', 'BNB', 'GBP', 8, 8, 2
  )

  new_ticker(
    'binancecoin',
    'paxos-standard',
    'Exchanges::Binance',
    'BNBPAX', 'BNB', 'PAX', 8, 8, 2
  )

  new_ticker(
    'bitcoin',
    'AUD.FOREX',
    'Exchanges::Binance',
    'BTCAUD', 'BTC', 'AUD', 8, 8, 2
  )

  new_ticker(
    'bitcoin',
    'GBP.FOREX',
    'Exchanges::Binance',
    'BTCGBP', 'BTC', 'GBP', 8, 8, 2
  )

  new_ticker(
    'bitcoin',
    'rupiah-token',
    'Exchanges::Binance',
    'BTCIDRT', 'BTC', 'IDRT', 8, 8, 2
  )

  new_ticker(
    'bitcoin',
    'RUB.FOREX',
    'Exchanges::Binance',
    'BTCRUB', 'BTC', 'RUB', 8, 8, 2
  )

  new_ticker(
    'bittorrent',
    'EUR.FOREX',
    'Exchanges::Binance',
    'BTTEUR', 'BTT', 'EUR', 8, 8, 2
  )

  new_ticker(
    'pancakeswap-token',
    'binance-peg-busd',
    'Exchanges::Binance',
    'CAKEBUSD', 'CAKE', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'nervos-network',
    'binance-peg-busd',
    'Exchanges::Binance',
    'CKBBUSD', 'CKB', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'digibyte',
    'binance-peg-busd',
    'Exchanges::Binance',
    'DGBBUSD', 'DGB', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'dock',
    'tether',
    'Exchanges::Binance',
    'DOCKUSDT', 'DOCK', 'USDT', 8, 8, 2
  )

  new_ticker(
    'dodo',
    'binance-peg-busd',
    'Exchanges::Binance',
    'DODOBUSD', 'DODO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'dogecoin',
    'GBP.FOREX',
    'Exchanges::Binance',
    'DOGEGBP', 'DOGE', 'GBP', 8, 8, 2
  )

  new_ticker(
    'polkadot',
    'GBP.FOREX',
    'Exchanges::Binance',
    'DOTGBP', 'DOT', 'GBP', 8, 8, 2
  )

  new_ticker(
    'enjincoin',
    'binance-peg-busd',
    'Exchanges::Binance',
    'ENJBUSD', 'ENJ', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'eos',
    'binance-peg-busd',
    'Exchanges::Binance',
    'EOBUSD', 'EOS', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'ethereum',
    'AUD.FOREX',
    'Exchanges::Binance',
    'ETHAUD', 'ETH', 'AUD', 8, 8, 2
  )

  new_ticker(
    'ethereum',
    'GBP.FOREX',
    'Exchanges::Binance',
    'ETHGBP', 'ETH', 'GBP', 8, 8, 2
  )

  new_ticker(
    'ethereum',
    'RUB.FOREX',
    'Exchanges::Binance',
    'ETHRUB', 'ETH', 'RUB', 8, 8, 2
  )

  new_ticker(
    'flow',
    'binance-peg-busd',
    'Exchanges::Binance',
    'FLOWBUSD', 'FLOW', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'zelcash',
    'binance-peg-busd',
    'Exchanges::Binance',
    'FLUXBUSD', 'FLUX', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'fantom',
    'AUD.FOREX',
    'Exchanges::Binance',
    'FTMAUD', 'FTM', 'AUD', 8, 8, 2
  )

  new_ticker(
    'fantom',
    'binance-peg-busd',
    'Exchanges::Binance',
    'FTMBUSD', 'FTM', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'fantom',
    'tether',
    'Exchanges::Binance',
    'FTMUSDT', 'FTM', 'USDT', 8, 8, 2
  )

  new_ticker(
    'gmx',
    'binance-peg-busd',
    'Exchanges::Binance',
    'GMXBUSD', 'GMX', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'helium',
    'tether',
    'Exchanges::Binance',
    'HNTUSDT', 'HNT', 'USDT', 8, 8, 2
  )

  new_ticker(
    'iotex',
    'binance-peg-busd',
    'Exchanges::Binance',
    'IOTXBUSD', 'IOTX', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'kusama',
    'binance-peg-busd',
    'Exchanges::Binance',
    'KSMBUSD', 'KSM', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'linear',
    'tether',
    'Exchanges::Binance',
    'LINAUSDT', 'LINA', 'USDT', 8, 8, 2
  )

  new_ticker(
    'chainlink',
    'GBP.FOREX',
    'Exchanges::Binance',
    'LINKGBP', 'LINK', 'GBP', 8, 8, 2
  )

  new_ticker(
    'loopring',
    'binance-peg-busd',
    'Exchanges::Binance',
    'LRCBUSD', 'LRC', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'litecoin',
    'GBP.FOREX',
    'Exchanges::Binance',
    'LTCGBP', 'LTC', 'GBP', 8, 8, 2
  )

  new_ticker(
    'litecoin',
    'RUB.FOREX',
    'Exchanges::Binance',
    'LTCRUB', 'LTC', 'RUB', 8, 8, 2
  )

  new_ticker(
    'lto-network',
    'binance-peg-busd',
    'Exchanges::Binance',
    'LTOBUSD', 'LTO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'terra-luna-2',
    'binance-peg-busd',
    'Exchanges::Binance',
    'LUNABUSD', 'LUNA', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'measurable-data-token',
    'binance-peg-busd',
    'Exchanges::Binance',
    'MDTBUSD', 'MDT', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'maker',
    'binance-peg-busd',
    'Exchanges::Binance',
    'MKRBUSD', 'MKR', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'nano',
    'binance-peg-busd',
    'Exchanges::Binance',
    'XNOBUSD', 'XNO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'neo',
    'binance-peg-busd',
    'Exchanges::Binance',
    'NEOBUSD', 'NEO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'nexo',
    'binance-peg-busd',
    'Exchanges::Binance',
    'NEXOBUSD', 'NEXO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'ocean-protocol',
    'tether',
    'Exchanges::Binance',
    'OCEANUSDT', 'OCEAN', 'USDT', 8, 8, 2
  )

  new_ticker(
    'omisego',
    'binance-peg-busd',
    'Exchanges::Binance',
    'OMGBUSD', 'OMG', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'osmosis',
    'binance-peg-busd',
    'Exchanges::Binance',
    'OSMOBUSD', 'OSMO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'polygon-ecosystem-token',
    'GBP.FOREX',
    'Exchanges::Binance',
    'POLGBP', 'POL', 'GBP', 8, 8, 2
  )

  new_ticker(
    'polkastarter',
    'binance-peg-busd',
    'Exchanges::Binance',
    'POLSBUSD', 'POLS', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'polkastarter',
    'tether',
    'Exchanges::Binance',
    'POLSUSDT', 'POLS', 'USDT', 8, 8, 2
  )

  new_ticker(
    'quant-network',
    'binance-peg-busd',
    'Exchanges::Binance',
    'QNTBUSD', 'QNT', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'reef',
    'tether',
    'Exchanges::Binance',
    'REEFUSDT', 'REEF', 'USDT', 8, 8, 2
  )

  new_ticker(
    'republic-protocol',
    'tether',
    'Exchanges::Binance',
    'RENUSDT', 'REN', 'USDT', 8, 8, 2
  )

  new_ticker(
    'render-token',
    'tether',
    'Exchanges::Binance',
    'RNDRUSDT', 'RNDR', 'USDT', 8, 8, 2
  )

  new_ticker(
    'oasis-network',
    'binance-peg-busd',
    'Exchanges::Binance',
    'ROSEBUSD', 'ROSE', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'siacoin',
    'binancecoin',
    'Exchanges::Binance',
    'SCBNB', 'SC', 'BNB', 8, 8, 2
  )

  new_ticker(
    'siacoin',
    'binance-peg-busd',
    'Exchanges::Binance',
    'SCBUSD', 'SC', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'havven',
    'binance-peg-busd',
    'Exchanges::Binance',
    'SNXBUSD', 'SNX', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'solana',
    'GBP.FOREX',
    'Exchanges::Binance',
    'SOLGBP', 'SOL', 'GBP', 8, 8, 2
  )

  new_ticker(
    'swipe',
    'binance-peg-busd',
    'Exchanges::Binance',
    'SXPBUSD', 'SXP', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'usd-coin',
    'binance-peg-busd',
    'Exchanges::Binance',
    'USDCBUSD', 'USDC', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'vechain',
    'binance-peg-busd',
    'Exchanges::Binance',
    'VETBUSD', 'VET', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'vechain',
    'GBP.FOREX',
    'Exchanges::Binance',
    'VETGBP', 'VET', 'GBP', 8, 8, 2
  )

  new_ticker(
    'waves',
    'tether',
    'Exchanges::Binance',
    'WAVESUSDT', 'WAVES', 'USDT', 8, 8, 2
  )

  new_ticker(
    'monero',
    'binancecoin',
    'Exchanges::Binance',
    'XMRBNB', 'XMR', 'BNB', 8, 8, 2
  )

  new_ticker(
    'monero',
    'ethereum',
    'Exchanges::Binance',
    'XMRETH', 'XMR', 'ETH', 8, 8, 2
  )

  new_ticker(
    'monero',
    'binance-peg-busd',
    'Exchanges::Binance',
    'XMRBUSD', 'XMR', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'monero',
    'tether',
    'Exchanges::Binance',
    'XMRUSDT', 'XMR', 'USDT', 8, 8, 2
  )

  new_ticker(
    'ripple',
    'GBP.FOREX',
    'Exchanges::Binance',
    'XRPGBP', 'XRP', 'GBP', 8, 8, 2
  )

  new_ticker(
    'tezos',
    'binance-peg-busd',
    'Exchanges::Binance',
    'XTZBUSD', 'XTZ', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'zencash',
    'binance-peg-busd',
    'Exchanges::Binance',
    'ZENBUSD', 'ZEN', 'BUSD', 8, 8, 2
  )

  new_ticker(
    '0x',
    'binance-peg-busd',
    'Exchanges::Binance',
    'ZRXBUSD', 'ZRX', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'amp-token',
    'USD.FOREX',
    'Exchanges::BinanceUs',
    'AMPUSD', 'AMP', 'USD', 8, 8, 2
  )

  new_ticker(
    'fantom',
    'USD.FOREX',
    'Exchanges::BinanceUs',
    'FTMUSD', 'FTM', 'USD', 8, 8, 2
  )

  new_ticker(
    'helium',
    'USD.FOREX',
    'Exchanges::BinanceUs',
    'HNTUSD', 'HNT', 'USD', 8, 8, 2
  )

  new_ticker(
    'helium',
    'tether',
    'Exchanges::BinanceUs',
    'HNTUSDT', 'HNT', 'USDT', 8, 8, 2
  )

  new_ticker(
    'polygon-ecosystem-token',
    'USD.FOREX',
    'Exchanges::BinanceUs',
    'POLUSD', 'POL', 'USD', 8, 8, 2
  )

  new_ticker(
    'enjincoin',
    'USD.FOREX',
    'Exchanges::Coinbase',
    'ENJ-USD', 'ENJ', 'USD', 8, 8, 2
  )

  new_ticker(
    'movement',
    'usd-coin',
    'Exchanges::Coinbase',
    'MOVE-USDC', 'MOVE', 'USDC', 8, 8, 2
  )

  new_ticker(
    'nucypher',
    'USD.FOREX',
    'Exchanges::Coinbase',
    'NU-USD', 'NU', 'USD', 8, 8, 2
  )

  new_ticker(
    'ethos',
    'USD.FOREX',
    'Exchanges::Coinbase',
    'VGX-USD', 'VGX', 'USD', 8, 8, 2
  )

  new_ticker(
    'wrapped-bitcoin',
    'USD.FOREX',
    'Exchanges::Coinbase',
    'WBTC-USD', 'WBTC', 'USD', 8, 8, 2
  )

  new_ticker(
    'polkadot',
    'AUD.FOREX',
    'Exchanges::Kraken',
    'DOTAUD', 'DOT', 'AUD', 8, 8, 2
  )

  new_ticker(
    'fantom',
    'EUR.FOREX',
    'Exchanges::Kraken',
    'FTMEUR', 'FTM', 'EUR', 8, 8, 2
  )

  new_ticker(
    'polygon-ecosystem-token',
    'GBP.FOREX',
    'Exchanges::Kraken',
    'POLGBP', 'POL', 'GBP', 8, 8, 2
  )

  new_ticker(
    'polygon-ecosystem-token',
    'tether',
    'Exchanges::Kraken',
    'POLUSDT', 'POL', 'USDT', 8, 8, 2
  )

  new_ticker(
    'polygon-ecosystem-token',
    'bitcoin',
    'Exchanges::Kraken',
    'POLXBT', 'POL', 'XBT', 8, 8, 2
  )

  new_ticker(
    'waves',
    'USD.FOREX',
    'Exchanges::Kraken',
    'WAVESUSD', 'WAVES', 'USD', 8, 8, 2
  )

  new_ticker(
    'tezos',
    'AUD.FOREX',
    'Exchanges::Kraken',
    'XTZAUD', 'XTZ', 'AUD', 8, 8, 2
  )

  new_ticker(
    'tezos',
    'GBP.FOREX',
    'Exchanges::Kraken',
    'XTZGBP', 'XTZ', 'GBP', 8, 8, 2
  )

  new_ticker(
    'aragon',
    'EUR.FOREX',
    'Exchanges::Kraken',
    'ANTEUR', 'ANT', 'EUR', 8, 8, 2
  )

  new_ticker(
    'aragon',
    'USD.FOREX',
    'Exchanges::Kraken',
    'ANTUSD', 'ANT', 'USD', 8, 8, 2
  )

  new_ticker(
    'fantom',
    'USD.FOREX',
    'Exchanges::Kraken',
    'FTMUSD', 'FTM', 'USD', 8, 8, 2
  )

  Exchange::SyncAllTickersAndAssetsJob.perform_later
end

def new_ticker(
  base_asset_external_id,
  quote_asset_external_id,
  exchange_type,
  ticker_name,
  base,
  quote,
  base_decimals,
  quote_decimals,
  price_decimals
)
  fiat_currency = Fiat.currencies.find { |c| c[:external_id] == base_asset_external_id }
  if fiat_currency.present?
    base_asset = if base_asset_external_id.in?(Asset.pluck(:external_id))
                   Asset.find_by(external_id: base_asset_external_id)
                 else
                   Asset.create!(fiat_currency)
                 end
  else
    base_asset = Asset.find_or_create_by!(external_id: base_asset_external_id, category: 'Cryptocurrency')
    Asset::FetchDataFromCoingeckoJob.perform_later(base_asset)
  end
  ExchangeAsset.find_or_create_by!(
    asset_id: base_asset.id,
    exchange_id: Exchange.find_by(type: exchange_type).id
  )
  fiat_currency = Fiat.currencies.find { |c| c[:external_id] == quote_asset_external_id }
  if fiat_currency.present?
    quote_asset = if quote_asset_external_id.in?(Asset.pluck(:external_id))
                    Asset.find_by(external_id: quote_asset_external_id)
                  else
                    Asset.create!(fiat_currency)
                  end
  else
    quote_asset = Asset.find_or_create_by!(external_id: quote_asset_external_id, category: 'Cryptocurrency')
    Asset::FetchDataFromCoingeckoJob.perform_later(quote_asset)
  end
  ExchangeAsset.find_or_create_by!(
    asset_id: quote_asset.id,
    exchange_id: Exchange.find_by(type: exchange_type).id
  )
  ticker = Ticker.find_by(base_asset: base_asset, quote_asset: quote_asset, exchange: Exchange.find_by(type: exchange_type))
  if ticker.present?
    ticker.update!(
      base_decimals: ticker.base_decimals || base_decimals,
      quote_decimals: ticker.quote_decimals || quote_decimals,
      price_decimals: ticker.price_decimals || price_decimals,
      minimum_base_size: ticker.minimum_base_size || 0,
      minimum_quote_size: ticker.minimum_quote_size || 0
    )
  else
    puts "Creating ticker #{ticker_name} for #{base_asset_external_id} and #{quote_asset_external_id} on #{exchange_type}"
    Ticker.create!(
      base_asset: base_asset,
      quote_asset: quote_asset,
      exchange: Exchange.find_by(type: exchange_type),
      ticker: ticker_name,
      base: base,
      quote: quote,
      base_decimals: base_decimals,
      quote_decimals: quote_decimals,
      price_decimals: price_decimals,
      minimum_base_size: 0,
      minimum_quote_size: 0
    )
  end
end
