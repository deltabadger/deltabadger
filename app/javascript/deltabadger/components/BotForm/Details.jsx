import React from 'react'
import I18n from 'i18n-js'
import { RawHTML } from '../RawHtml'

export const Details = () => {
  return (
    <div className="db-bots__item db-bots__item--data">
      <div className="db-bots__tabs" id="botFormInfo" role="tablist">
        <div className="nav-item">
          <a className="nav-link active" id="botFormInfoTab" data-toggle="tab" href="#botFormInfoTab"  role="tab" aria-controls="botFormInfoTab"  aria-selected="false">Info</a>
        </div>
      </div>
      <div className="tab-content" id="botFormInfo">
        <div className="legacy-tab active" id="botFormInfoTab" role="tabpanel" aria-labelledby="botFormInfoTab">
          <div  className="legacy-tab__section db-showif--pick-exchange">
            <RawHTML>
              {I18n.t('bot.details.info.what_is_dca_html')}
            </RawHTML>
          </div>
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
      </div>
    </div>
  )
}
