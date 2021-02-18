import React from 'react'
import I18n from 'i18n-js'
import { RawHTML } from '../RawHtml'

const exchanges = {
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
}

export const Instructions = ({ exchangeName }) => {
  const exchange = exchanges[exchangeName.toLowerCase()]
  if (!exchange) return null

  const { name, url, translation_key } = exchange

  const anchor = `<a href="${url}" target="_blank" rel="nofollow">${name}</a>`

  return (
    <div className="db-exchange-instructions">
      <div className="alert alert-success mx-0" role="alert">
        <b className="alert-heading mb-2">{I18n.t('bots.setup.how_to_get_keys', {exchange: name})}</b>
        <hr/>
        <RawHTML>{I18n.t('bots.setup.' + translation_key + '.instructions_html', { exchange_link: anchor})}</RawHTML>
      </div>
    </div>
  )
}
