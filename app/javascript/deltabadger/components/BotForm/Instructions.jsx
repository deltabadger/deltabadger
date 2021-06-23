import React from 'react'
import I18n from 'i18n-js'
import { RawHTML } from '../RawHtml'
import { getExchange } from '../../lib/exchanges'

export const Instructions = ({ exchangeName }) => {
  const exchange = getExchange(exchangeName)
  if (!exchange) return null

  const { name, url, translation_key } = exchange

  const anchor = `<a href="${url}" target="_blank" rel="nofollow">${name}</a>`

  return (
    <div className="db-exchange-instructions">
      <div className="alert alert-success" role="alert">
        <div className="alert__regular-text">
          <b className="alert-heading mb-2">{I18n.t('bots.setup.how_to_get_keys', {exchange: name})}</b>
          <hr/>
          <RawHTML>{I18n.t('bots.setup.' + translation_key + '.instructions_html', { exchange_link: anchor})}</RawHTML>
        </div>
      </div>
    </div>
  )
}
