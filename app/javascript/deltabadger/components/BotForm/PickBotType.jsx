import React from 'react'
import I18n from 'i18n-js'
import { Breadcrumbs } from './Breadcrumbs'
import { Progressbar } from './Progressbar'

export const PickBotType = ({ handleSubmit, showWebhookButton }) => {
  return (
    <div className="db-bots__item db-bot db-bot--pick-exchange db-bot--active">
      <div className="db-bot__header">
        <Breadcrumbs step={0} />
        <div className="db-bot__infotext" />
      </div>
      <Progressbar value={0} />
      <div className="row db-bot__types">
        <div
          className="db-bot__types__item db-bot__types__item--dca"
          onClick={() => handleSubmit('trading')}
        >
          <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
            <path className="stroke-1" stroke="var(--pickExchangeItemTxt)" strokeLinecap="round" strokeWidth="2" d="M3 16h4c.3 0 .5-.2.5-.5v-3c0-.3.2-.5.5-.5h4c.3 0 .5-.2.5-.5v-3c0-.3.2-.5.5-.5h4"/>
            <path className="fill-1" fill="var(--pickExchangeItemTxt)" fillRule="evenodd" d="M19 7a1 1 0 1 0 0 2 1 1 0 0 0 0-2Zm3 1a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z" clipRule="evenodd"/>
          </svg>
          {I18n.t('bots.buttons.dollar_cost_averaging')}
        </div>
        <div
          className="db-bot__types__item db-bot__types__item--aw"
          onClick={() => handleSubmit('withdrawal')}
        >
          <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
            <path className="stroke-1" stroke="var(--pickExchangeItemTxt)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="m18 13-6 6-6-6M12 18V6"/>
          </svg>
          {I18n.t('bots.buttons.automatic_withdrawal')}
        </div>
        <div
          className={`db-bot__types__item db-bot__types__item--wh ${showWebhookButton ? '' : 'bot__types__item--inactive'}`}
          onClick={() => showWebhookButton ? handleSubmit('webhook') : null}
        >
          <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
            <path className="fill-1" fill="var(--pickExchangeItemTxt)" fillRule="evenodd" d="M12 15a1 1 0 1 0 0 2 1 1 0 0 0 0-2Zm3 1a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z" clipRule="evenodd"/>
            <path className="stroke-1" stroke="var(--pickExchangeItemTxt)" strokeLinecap="round" strokeWidth="2" d="M7.8 11.8a6 6 0 0 1 8.4 0M5 9c3.8-4 10.2-4 14 0"/>
          </svg>
          {I18n.t('bots.buttons.webhook')}
        </div>
      </div>
    </div>
  )
}
