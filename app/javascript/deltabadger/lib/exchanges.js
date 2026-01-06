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
  'kraken': {
      name: 'Kraken',
      url: 'https://r.kraken.com/deltabadger',
      translation_key: 'kraken',
  },
  'coinbase': {
      name: 'Coinbase',
      url: 'https://www.coinbase.com/advanced-trade',
      translation_key: 'coinbase',
  },
}

export const getExchange = (exchangeName, type) => {
  let exchange = {...exchanges[exchangeName.toLowerCase()]}
  if (type === 'withdrawal' || type === 'withdrawal_address') {
    exchange.translation_key = type + '.' + exchange.translation_key
  }

  return exchange
}
