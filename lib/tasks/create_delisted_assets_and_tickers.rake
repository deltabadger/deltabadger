desc 'rake task to create delisted assets and tickers'
task create_delisted_assets_and_tickers: :environment do
  new_ticker(
    'cardano', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Binance',
    'ADAGBP', 'ADA', 'GBP', 8, 8, 2
  )

  new_ticker(
    'audius', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'AUDIOBUSD', 'AUDIO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'axie-infinity', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'AXSBUSD', 'AXS', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'binancecoin', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Binance',
    'BNBGBP', 'BNB', 'GBP', 8, 8, 2
  )

  new_ticker(
    'binancecoin', 'Cryptocurrency',
    'paxos-standard', 'Cryptocurrency',
    'Exchanges::Binance',
    'BNBPAX', 'BNB', 'PAX', 8, 8, 2
  )

  new_ticker(
    'bitcoin', 'Cryptocurrency',
    'AUD.FOREX', 'Currency',
    'Exchanges::Binance',
    'BTCAUD', 'BTC', 'AUD', 8, 8, 2
  )

  new_ticker(
    'bitcoin', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Binance',
    'BTCGBP', 'BTC', 'GBP', 8, 8, 2
  )

  new_ticker(
    'bitcoin', 'Cryptocurrency',
    'rupiah-token', 'Cryptocurrency',
    'Exchanges::Binance',
    'BTCIDRT', 'BTC', 'IDRT', 8, 8, 2
  )

  new_ticker(
    'bitcoin', 'Cryptocurrency',
    'RUB.FOREX', 'Currency',
    'Exchanges::Binance',
    'BTCRUB', 'BTC', 'RUB', 8, 8, 2
  )

  new_ticker(
    'bittorrent', 'Cryptocurrency',
    'EUR.FOREX', 'Currency',
    'Exchanges::Binance',
    'BTTEUR', 'BTT', 'EUR', 8, 8, 2
  )

  new_ticker(
    'pancakeswap-token', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'CAKEBUSD', 'CAKE', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'nervos-network', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'CKBBUSD', 'CKB', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'digibyte', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'DGBBUSD', 'DGB', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'dock', 'Cryptocurrency',
    'tether', 'Cryptocurrency',
    'Exchanges::Binance',
    'DOCKUSDT', 'DOCK', 'USDT', 8, 8, 2
  )

  new_ticker(
    'dodo', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'DODOBUSD', 'DODO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'dogecoin', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Binance',
    'DOGEGBP', 'DOGE', 'GBP', 8, 8, 2
  )

  new_ticker(
    'polkadot', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Binance',
    'DOTGBP', 'DOT', 'GBP', 8, 8, 2
  )

  new_ticker(
    'enjincoin', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'ENJBUSD', 'ENJ', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'eos', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'EOBUSD', 'EOS', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'ethereum', 'Cryptocurrency',
    'AUD.FOREX', 'Currency',
    'Exchanges::Binance',
    'ETHAUD', 'ETH', 'AUD', 8, 8, 2
  )

  new_ticker(
    'ethereum', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Binance',
    'ETHGBP', 'ETH', 'GBP', 8, 8, 2
  )

  new_ticker(
    'ethereum', 'Cryptocurrency',
    'RUB.FOREX', 'Currency',
    'Exchanges::Binance',
    'ETHRUB', 'ETH', 'RUB', 8, 8, 2
  )

  new_ticker(
    'flow', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'FLOWBUSD', 'FLOW', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'zelcash', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'FLUXBUSD', 'FLUX', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'fantom', 'Cryptocurrency',
    'AUD.FOREX', 'Currency',
    'Exchanges::Binance',
    'FTMAUD', 'FTM', 'AUD', 8, 8, 2
  )

  new_ticker(
    'fantom', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'FTMBUSD', 'FTM', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'fantom', 'Cryptocurrency',
    'RUB.FOREX', 'Currency',
    'Exchanges::Binance',
    'FTMRUB', 'FTM', 'RUB', 8, 8, 2
  )

  new_ticker(
    'gmx', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'GMXBUSD', 'GMX', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'helium', 'Cryptocurrency',
    'tether', 'Cryptocurrency',
    'Exchanges::Binance',
    'HNTUSDT', 'HNT', 'USDT', 8, 8, 2
  )

  new_ticker(
    'iotex', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'IOTXBUSD', 'IOTX', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'kusama', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'KSMBUSD', 'KSM', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'linear', 'Cryptocurrency',
    'tether', 'Cryptocurrency',
    'Exchanges::Binance',
    'LINAUSDT', 'LINA', 'USDT', 8, 8, 2
  )

  new_ticker(
    'chainlink', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Binance',
    'LINKGBP', 'LINK', 'GBP', 8, 8, 2
  )

  new_ticker(
    'loopring', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'LRCBUSD', 'LRC', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'litecoin', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Binance',
    'LTCGBP', 'LTC', 'GBP', 8, 8, 2
  )

  new_ticker(
    'litecoin', 'Cryptocurrency',
    'RUB.FOREX', 'Currency',
    'Exchanges::Binance',
    'LTCRUB', 'LTC', 'RUB', 8, 8, 2
  )

  new_ticker(
    'lto-network', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'LTOBUSD', 'LTO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'terra-luna-2', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'LUNABUSD', 'LUNA', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'measurable-data-token', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'MDTBUSD', 'MDT', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'maker', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'MKRBUSD', 'MKR', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'nano', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'XNOBUSD', 'XNO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'neo', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'NEOBUSD', 'NEO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'nexo', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'NEXOBUSD', 'NEXO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'ocean-protocol', 'Cryptocurrency',
    'tether', 'Cryptocurrency',
    'Exchanges::Binance',
    'OCEANUSDT', 'OCEAN', 'USDT', 8, 8, 2
  )

  new_ticker(
    'omisego', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'OMGBUSD', 'OMG', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'osmosis', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'OSMOBUSD', 'OSMO', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'polygon-ecosystem-token', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Binance',
    'POLGBP', 'POL', 'GBP', 8, 8, 2
  )

  new_ticker(
    'polkastarter', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'POLSBUSD', 'POLS', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'polkastarter', 'Cryptocurrency',
    'tether', 'Cryptocurrency',
    'Exchanges::Binance',
    'POLSUSDT', 'POLS', 'USDT', 8, 8, 2
  )

  new_ticker(
    'quant-network', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'QNTBUSD', 'QNT', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'reef', 'Cryptocurrency',
    'tether', 'Cryptocurrency',
    'Exchanges::Binance',
    'REEFUSDT', 'REEF', 'USDT', 8, 8, 2
  )

  new_ticker(
    'republic-protocol', 'Cryptocurrency',
    'tether', 'Cryptocurrency',
    'Exchanges::Binance',
    'RENUSDT', 'REN', 'USDT', 8, 8, 2
  )

  new_ticker(
    'render-token', 'Cryptocurrency',
    'tether', 'Cryptocurrency',
    'Exchanges::Binance',
    'RNDRUSDT', 'RNDR', 'USDT', 8, 8, 2
  )

  new_ticker(
    'oasis-network', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'ROSEBUSD', 'ROSE', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'siacoin', 'Cryptocurrency',
    'binancecoin', 'Cryptocurrency',
    'Exchanges::Binance',
    'SCBNB', 'SC', 'BNB', 8, 8, 2
  )

  new_ticker(
    'siacoin', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'SCBUSD', 'SC', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'havven', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'SNXBUSD', 'SNX', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'solana', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Binance',
    'SOLGBP', 'SOL', 'GBP', 8, 8, 2
  )

  new_ticker(
    'swipe', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'SXPBUSD', 'SXP', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'usd-coin', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'USDCBUSD', 'USDC', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'vechain', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'VETBUSD', 'VET', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'vechain', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Binance',
    'VETGBP', 'VET', 'GBP', 8, 8, 2
  )

  new_ticker(
    'waves', 'Cryptocurrency',
    'tether', 'Cryptocurrency',
    'Exchanges::Binance',
    'WAVESUSDT', 'WAVES', 'USDT', 8, 8, 2
  )

  new_ticker(
    'monero', 'Cryptocurrency',
    'binancecoin', 'Cryptocurrency',
    'Exchanges::Binance',
    'XMRBNB', 'XMR', 'BNB', 8, 8, 2
  )

  new_ticker(
    'monero', 'Cryptocurrency',
    'ethereum', 'Cryptocurrency',
    'Exchanges::Binance',
    'XMRETH', 'XMR', 'ETH', 8, 8, 2
  )

  new_ticker(
    'monero', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'XMRBUSD', 'XMR', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'monero', 'Cryptocurrency',
    'tether', 'Cryptocurrency',
    'Exchanges::Binance',
    'XMRUSDT', 'XMR', 'USDT', 8, 8, 2
  )

  new_ticker(
    'ripple', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Binance',
    'XRPGBP', 'XRP', 'GBP', 8, 8, 2
  )

  new_ticker(
    'tezos', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'XTZBUSD', 'XTZ', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'zencash', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'ZENBUSD', 'ZEN', 'BUSD', 8, 8, 2
  )

  new_ticker(
    '0x', 'Cryptocurrency',
    'binance-peg-busd', 'Cryptocurrency',
    'Exchanges::Binance',
    'ZRXBUSD', 'ZRX', 'BUSD', 8, 8, 2
  )

  new_ticker(
    'amp-token', 'Cryptocurrency',
    'USD.FOREX', 'Currency',
    'Exchanges::BinanceUs',
    'AMPUSD', 'AMP', 'USD', 8, 8, 2
  )

  new_ticker(
    'fantom', 'Cryptocurrency',
    'USD.FOREX', 'Currency',
    'Exchanges::BinanceUs',
    'FTMUSD', 'FTM', 'USD', 8, 8, 2
  )

  new_ticker(
    'helium', 'Cryptocurrency',
    'USD.FOREX', 'Currency',
    'Exchanges::BinanceUs',
    'HNTUSD', 'HNT', 'USD', 8, 8, 2
  )

  new_ticker(
    'helium', 'Cryptocurrency',
    'tether', 'Cryptocurrency',
    'Exchanges::BinanceUs',
    'HNTUSDT', 'HNT', 'USDT', 8, 8, 2
  )

  new_ticker(
    'polygon-ecosystem-token', 'Cryptocurrency',
    'USD.FOREX', 'Currency',
    'Exchanges::BinanceUs',
    'POLUSD', 'POL', 'USD', 8, 8, 2
  )

  new_ticker(
    'enjincoin', 'Cryptocurrency',
    'USD.FOREX', 'Currency',
    'Exchanges::Coinbase',
    'ENJ-USD', 'ENJ', 'USD', 8, 8, 2
  )

  new_ticker(
    'movement', 'Cryptocurrency',
    'usd-coin', 'Cryptocurrency',
    'Exchanges::Coinbase',
    'MOVE-USDC', 'MOVE', 'USDC', 8, 8, 2
  )

  new_ticker(
    'nucypher', 'Cryptocurrency',
    'USD.FOREX', 'Currency',
    'Exchanges::Coinbase',
    'NU-USD', 'NU', 'USD', 8, 8, 2
  )

  new_ticker(
    'ethos', 'Cryptocurrency',
    'USD.FOREX', 'Currency',
    'Exchanges::Coinbase',
    'VGX-USD', 'VGX', 'USD', 8, 8, 2
  )

  new_ticker(
    'wrapped-bitcoin', 'Cryptocurrency',
    'USD.FOREX', 'Currency',
    'Exchanges::Coinbase',
    'WBTC-USD', 'WBTC', 'USD', 8, 8, 2
  )

  new_ticker(
    'polkadot', 'Cryptocurrency',
    'AUD.FOREX', 'Currency',
    'Exchanges::Kraken',
    'DOTAUD', 'DOT', 'AUD', 8, 8, 2
  )

  new_ticker(
    'fantom', 'Cryptocurrency',
    'EUR.FOREX', 'Currency',
    'Exchanges::Kraken',
    'FTMEUR', 'FTM', 'EUR', 8, 8, 2
  )

  new_ticker(
    'polygon-ecosystem-token', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Kraken',
    'POLGBP', 'POL', 'GBP', 8, 8, 2
  )

  new_ticker(
    'polygon-ecosystem-token', 'Cryptocurrency',
    'tether', 'Cryptocurrency',
    'Exchanges::Kraken',
    'POLUSDT', 'POL', 'USDT', 8, 8, 2
  )

  new_ticker(
    'polygon-ecosystem-token', 'Cryptocurrency',
    'bitcoin', 'Cryptocurrency',
    'Exchanges::Kraken',
    'POLXBT', 'POL', 'XBT', 8, 8, 2
  )

  new_ticker(
    'waves', 'Cryptocurrency',
    'USD.FOREX', 'Currency',
    'Exchanges::Kraken',
    'WAVESUSD', 'WAVES', 'USD', 8, 8, 2
  )

  new_ticker(
    'tezos', 'Cryptocurrency',
    'AUD.FOREX', 'Currency',
    'Exchanges::Kraken',
    'XTZAUD', 'XTZ', 'AUD', 8, 8, 2
  )

  new_ticker(
    'tezos', 'Cryptocurrency',
    'GBP.FOREX', 'Currency',
    'Exchanges::Kraken',
    'XTZGBP', 'XTZ', 'GBP', 8, 8, 2
  )
end

def new_ticker(
  base_asset_external_id,
  base_asset_category,
  quote_asset_external_id,
  quote_asset_category,
  exchange_type,
  _ticker_name,
  base,
  quote,
  base_decimals,
  quote_decimals,
  price_decimals
)
  base_asset = Asset.find_or_create_by!(external_id: base_asset_external_id, category: base_asset_category)
  Asset::FetchDataFromCoingeckoJob.perform_later(base_asset) if base_asset_category == 'Cryptocurrency'
  ExchangeAsset.find_or_create_by!(
    asset_id: base_asset.id,
    exchange_id: Exchange.find_by(type: exchange_type).id,
    available: false
  )
  quote_asset = Asset.find_or_create_by!(external_id: quote_asset_external_id, category: quote_asset_category)
  Asset::FetchDataFromCoingeckoJob.perform_later(quote_asset) if quote_asset_category == 'Cryptocurrency'
  ExchangeAsset.find_or_create_by!(
    asset_id: quote_asset.id,
    exchange_id: Exchange.find_by(type: exchange_type).id,
    available: false
  )
  ticker = Ticker.find_or_create_by!(
    base_asset: base_asset,
    quote_asset: quote_asset,
    exchange: Exchange.find_by(type: exchange_type)
  )
  ticker.update!(
    ticker: ticker.ticker || ticker_name,
    base: ticker.base || base,
    quote: ticker.quote || quote,
    base_decimals: ticker.base_decimals || base_decimals,
    quote_decimals: ticker.quote_decimals || quote_decimals,
    price_decimals: ticker.price_decimals || price_decimals,
    minimum_base_size: ticker.minimum_base_size || 0,
    minimum_quote_size: ticker.minimum_quote_size || 0,
    available: ticker.available || false
  )
end
