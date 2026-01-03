import React from 'react'
import I18n from 'i18n-js'
import { RawHTML } from '../RawHtml'

export const Info = ({ active }) => (
  <div className={`legacy-tab ${active ? 'active' : ''}`} id="info" role="tabpanel" aria-labelledby="info-tab">
    <RawHTML className="legacy-tab__section db-showif--pick-exchange">
      {I18n.t('bot.details.info.what_is_dca_html')}
    </RawHTML>
    <RawHTML className="db-bot-info--dca">
      {I18n.t('bot.details.info.smart_intervals_html')}
    </RawHTML>
    <RawHTML className="legacy-tab__section db-showif--setup db-bot-info--dca second-info-title">
      {I18n.t('bot.details.info.daily_weekly_monthly_html')}
    </RawHTML>
    <RawHTML className="legacy-tab__section db-showif--setup db-bot-info--withdrawal">
      {I18n.t('bot.details.info.withdrawal_html')}
    </RawHTML>
  </div>
)
