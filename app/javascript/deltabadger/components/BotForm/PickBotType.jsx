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
            <path className="stroke-1" stroke="var(--pickExchangeItemTxt)" strokeLinecap="round" strokeWidth="2" d="M4 16h3.5c.3 0 .5-.2.5-.5v-3c0-.3.2-.5.5-.5h3c.3 0 .5-.2.5-.5v-3c0-.3.2-.5.5-.5H16"/>
            <circle className="stroke-1" stroke="var(--pickExchangeItemTxt)" cx="18" cy="8" r="2" strokeWidth="2"/>
          </svg>
          {I18n.t('bot.buttons.dollar_cost_averaging')}
        </div>
        <div
          className="db-bot__types__item db-bot__types__item--aw"
          onClick={() => handleSubmit('withdrawal')}
        >
          <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
            <path className="stroke-1" stroke="var(--pickExchangeItemTxt)" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="m18 11-6 6-6-6M12 16V5"/>
          </svg>
          {I18n.t('bot.buttons.automatic_withdrawal')}
        </div>
        {/* <div
          className={`db-bot__types__item db-bot__types__item--wh ${showWebhookButton ? '' : 'db-bot__types__item--inactive'}`}
          onClick={() => showWebhookButton ? handleSubmit('webhook') : null}
        >
          <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                <path className="stroke-1" stroke="var(--pickExchangeItemTxt)" strokeLinecap="round" strokeWidth="2" d="M7.8 10.8a6 6 0 0 1 8.4 0M5 8c3.8-4 10.2-4 14 0"/>
                <circle className="stroke-1" cx="12" cy="15" r="2" stroke="var(--pickExchangeItemTxt)" strokeWidth="2"/>
              </svg>
          {I18n.t('bot.buttons.webhook')}
        </div> */}
        <a
          className="db-bot__types__item db-bot__types__item--move"
          href="mailto:jan@deltabadger.com?subject=Move my bot to another exchange"
        >
          {I18n.t('bot.buttons.move_existing_bot')}
        </a>
      </div>
    </div>
  )
}
