import React from 'react'
import I18n from 'i18n-js'
import { ExchangeButton } from '../buttons'
import { Breadcrumbs } from './Breadcrumbs'
import { Progressbar } from './Progressbar'

export const PickExchange = ({ handleSubmit, exchanges, type }) => {
  console.log("Exchanges:" + JSON.stringify(exchanges, null, 2));
  return (
    <div className="db-bots__item db-bot db-bot--pick-exchange db-bot--active">
      <div className="db-bot__header">
        <Breadcrumbs step={1} />
        <div className="db-bot__infotext" />
      </div>
      <Progressbar value={0} />
      <div className="db-bot__exchanges">
        <div className="db-bot__exchanges__item db-bot__exchanges__item--header">
          <div>{I18n.t('bots.fees')}</div>
          <div>{I18n.t('bots.maker_fee')}</div>
          <div>{I18n.t('bots.taker_fee')}</div>
          <div>{I18n.t('bots.withdrawal_fee')}</div>
        </div>
        {
          exchanges.map(e =>
            <ExchangeButton key={e.id} handleClick={handleSubmit} exchange={e} type={type}/>
          )
        }
        <a href="mailto:jan@deltabadger.com?subject=Exchange%20request" className="db-bot__exchanges__item db-bot__exchanges__item--link">{I18n.t('bots.buttons.request_exchange')}</a>
      </div>
      
    </div>
    
  )
}