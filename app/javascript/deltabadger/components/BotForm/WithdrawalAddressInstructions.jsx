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
    return I18n.lookup('bots.setup.' + translation_key + '.instructions_' + counter + '_html', I18n.locale)
  }

  const mergedInstruction = () => {
    let merged = '<ol>'
    let counter = 1
    while(hasNext(counter)){
      merged += `<li>${I18n.t('bots.setup.' + translation_key + '.instructions_' + counter + '_html', {exchange_link: anchor})}</li>`
      counter += 1
    }
    merged += '</ol>'

    return merged.replaceAll('\\n', '')
  }

  return (
    <div className="db-exchange-instructions">
      <div className="alert alert-success" role="alert">
        <div className="alert__regular-text">
          <b className="alert-heading mb-2">{`How to set up withdrawal bot:`}</b>
          <hr/>
          <ol>
            <li>Log in to your Kraken account.</li>
            <li>Go to <b>Funding</b> → <b>Withdraw</b>.</li>
            <li>Pick asset you are interested in.</li>
            <li>Add new withdrawal address or manage already created addresses.</li>
            <li>Copy description you assigned to withdrawal address in to the form above.</li>
          </ol>
        </div>
      </div>
    </div>
  )
}
