import React from 'react'
import I18n from 'i18n-js'
import { RawHTML } from '../RawHtml'

export const Details = () => {
  return (
    <div className="db-bots__item db-bots__item--data">
      <ul className="nav nav-tabs" id="botFormInfo" role="tablist">
        <li className="nav-item">
          <a className="nav-link active" id="botFormInfoTab" data-toggle="tab" href="#botFormInfoTab"  role="tab" aria-controls="botFormInfoTab"  aria-selected="false">Info</a>
        </li>
      </ul>
      <div className="tab-content" id="botFormInfo">
        <div className="tab-pane show active" id="botFormInfoTab" role="tabpanel" aria-labelledby="botFormInfoTab">
          <RawHTML className="db-showif db-showif--pick-exchange">
            {I18n.t('bots.details.info.exchanges_html')}
          </RawHTML>
          <RawHTML className="db-showif db-showif--setup">
            {I18n.t('bots.details.info.smart_intervals_html')}
          </RawHTML>
          <RawHTML>
            {I18n.t('bots.details.info.new_to_dca_html')}
          </RawHTML>
        </div>
      </div>
    </div>
  )
}
