import React from 'react'
import I18n from 'i18n-js'
import { RawHTML } from '../RawHtml'
import { getExchange } from '../../lib/exchanges'

export const Instructions = ({ exchangeName }) => {
  const exchange = getExchange(exchangeName)
  if (!exchange) return null

  const { name, url, translation_key } = exchange

  const anchor = `<a href="${url}" target="_blank" rel="nofollow">${name}</a>`

  const hasNextInner = (counter, inner_counter) => {
    return I18n.lookup('bots.setup.' + translation_key + '.instructions_' + counter + '_' + inner_counter + '_html', I18n.locale)
  }

  const hasNext = (counter, inner_counter = 1) => {
    return I18n.lookup('bots.setup.' + translation_key + '.instructions_' + counter + '_html', I18n.locale) ||
      hasNextInner(counter, inner_counter)
  }

  const mergedInstruction = () => {
    let merged = '<ol>'
    let counter = 1
    let inner_counter = 1
    while(hasNext(counter)){
      if (hasNextInner(counter, inner_counter)) {
        let tag = translation_key !== 'kraken' ? '<li>' : '\n'
        merged += `${tag}${I18n.t('bots.setup.' + translation_key + '.instructions_' + counter + '_' + inner_counter + '_html', {exchange_link: anchor})} <ul>`

        inner_counter += 1
        while (hasNextInner(counter, inner_counter)) {
          merged += `<li>${I18n.t('bots.setup.' + translation_key + '.instructions_' + counter + '_' + inner_counter + '_html', { exchange_link: anchor})} </li>`
          inner_counter += 1
        }
        let endTag = translation_key !== 'kraken' ? '</li>' : ''
        merged += `</ul>${endTag}`

        inner_counter = 1
        counter += 1
        continue
      }

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
          <b className="alert-heading mb-2">{I18n.t('bots.setup.how_to_get_keys', {exchange: name})}</b>
          <hr/>
          <RawHTML>{mergedInstruction()}</RawHTML>
        </div>
      </div>
    </div>
  )
}
