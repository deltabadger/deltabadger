import React from 'react'
import I18n from 'i18n-js'
import { RawHTML } from '../RawHtml'
import { getExchange } from '../../lib/exchanges'

export const WithdrawalAddressInstructions = ({ exchangeName, type }) => {
  const exchange = getExchange(exchangeName, type)
  if (!exchange) return null

  const { name, url, translation_key } = exchange

  const anchor = `<a href="${url}" target="_blank" rel="nofollow">${name}</a>`

  const hasNext = (counter) => {
    return I18n.lookup('bot.setup.' + translation_key + '.instructions_' + counter + '_html', I18n.locale)
  }

  const mergedInstruction = () => {
    let merged = '<ol>'
    let counter = 1
    while(hasNext(counter)){
      merged += `<li>${I18n.t('bot.setup.' + translation_key + '.instructions_' + counter + '_html', {exchange_link: anchor})}</li>`
      counter += 1
    }
    merged += '</ol>'

    return merged.replaceAll('\\n', '')
  }

  return (
    <div className="db-exchange-instructions">
      <div className="alert alert-success" role="alert">
        <div className="alert__regular-text">
          <div className="alert__heading">{I18n.t('bot.setup.how_to_add_withdrawal_bot')}</div>
          <RawHTML>{mergedInstruction()}</RawHTML>
        </div>
      </div>
    </div>
  )
}
