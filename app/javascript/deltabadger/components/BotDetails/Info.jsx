import React from 'react'
import I18n from 'i18n-js'
import { RawHTML } from '../RawHtml'

export const Info = ({ active }) => (
  <div className={`legacy-tab ${active ? 'active' : ''}`} id="info" role="tabpanel" aria-labelledby="info-tab">
    <RawHTML className="legacy-tab__section db-showif--pick-exchange">
      {I18n.t('bots.details.info.what_is_dca_html')}
    </RawHTML>
    <RawHTML className="db-bot-info--dca">
      {I18n.t('bots.details.info.smart_intervals_html')}
    </RawHTML>
    <RawHTML className="legacy-tab__section db-showif--setup db-bot-info--dca second-info-title">
      {I18n.t('bots.details.info.daily_weekly_monthly_html')}
    </RawHTML>
    <RawHTML className="legacy-tab__section db-showif--setup db-bot-info--withdrawal">
      {I18n.t('bots.details.info.withdrawal_html')}
    </RawHTML>
    <div className="db-bot-info--webhook">
      <RawHTML>
        {I18n.t('bots.details.info.webhook.info_html')}
      </RawHTML>
      <p>1. {I18n.t('bots.details.info.webhook.create_alert')}</p>
      <img src="https://deltabadger.com/app/webhook-tw-01.webp" alt="Create alert in Trading View" />
      <p>2. {I18n.t('bots.details.info.webhook.configure_signal')}</p>
      <img src="https://deltabadger.com/app/webhook-tw-02.webp" alt="Configure signal in Trading View" />
      <p>3. {I18n.t('bots.details.info.webhook.set_webhook')}</p>
      <img src="https://deltabadger.com/app/webhook-tw-03.webp" alt="Set webhook in Trading View" />
      <p>{I18n.t('bots.details.info.webhook.experimental')}</p>
    </div>
  </div>
)
