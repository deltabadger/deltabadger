import React from 'react'
import I18n from 'i18n-js'
import { RawHTML } from '../RawHtml'

export const Info = ({ active }) => (
  <div className={`pb-4 tab-pane ${active ? 'active' : ''}`} id="info" role="tabpanel" aria-labelledby="info-tab">
    <RawHTML className="db-showif db-showif--pick-exchange">
      {I18n.t('bots.details.info.exchanges_html')}
    </RawHTML>
    <RawHTML className="db-showif db-showif--setup db-bot-info--dca">
      {I18n.t('bots.details.info.smart_intervals_html')}
    </RawHTML>
    <RawHTML className="db-bot-info--dca">
      {I18n.t('bots.details.info.new_to_dca_html')}
    </RawHTML>
    <RawHTML className="db-bot-info--webhook">
      {I18n.t('bots.details.info.webhook_html')}
    </RawHTML>
  </div>
)
