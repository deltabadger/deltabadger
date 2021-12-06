export const exchanges = {
  'binance': {
      name: 'Binance',
      url: 'https://www.binance.com/en/register?ref=NUYVIP6R',
      translation_key: 'binance',
  },
  'binance.us': {
      name: 'Binance.US',
      url: 'https://www.binance.us/en/home',
      translation_key: 'binance',
  },
  'bitbay': {
      name: 'BitBay',
      url: 'https://auth.bitbay.net/ref/Hhb7ZrAv2GrA',
      translation_key: 'bitbay',
  },
  'kraken': {
      name: 'Kraken',
      url: 'https://r.kraken.com/deltabadger',
      translation_key: 'kraken',
  },
  'coinbase pro': {
      name: 'Coinbase',
      url: 'https://pro.coinbase.com/',
      translation_key: 'coinbase_pro',
  },
  'gemini': {
      name: 'Gemini',
      url: 'https://exchange.gemini.com/signin',
      translation_key: 'gemini',
  },
  'ftx': {
      name: 'FTX',
      url: 'https://ftx.com',
      translation_key: 'ftx',
  },
  'ftx.us': {
    name: 'FTX.US',
    url: 'https://ftx.us',
    translation_key  : 'ftx',
    },
  'bitso': {
    name: 'Bitso',
    url: 'https://bitso.com/',
    translation_key: 'bitso'
  },
  'kucoin': {
    name: 'KuCoin',
    url: 'https://kucoin.com/',
    translation_key: 'kucoin'
  },
  'bitfinex': {
    name: 'Bitfinex',
    url: 'https://bitfinex.com/',
    translation_key: 'bitfinex'
  },
  'bitstamp': {
    name: 'Bitstamp',
    url: 'https://bitstamp.net/',
    translation_key: 'bitstamp'
  },
  'probit': {
    name: 'ProBit Global',
    url: 'https://www.probit.com/',
    translation_key: 'probit'
  },
  'probit global': {
    name: 'ProBit Global',
    url: 'https://www.probit.com/',
    translation_key: 'probit'
  }
}

export const getExchange = (exchangeName, type) => {
  let exchange = {...exchanges[exchangeName.toLowerCase()]}
  if (type === 'withdrawal' || type === 'withdrawal_address') {
    exchange.translation_key = type + '.' + exchange.translation_key
  }

  return exchange
}
