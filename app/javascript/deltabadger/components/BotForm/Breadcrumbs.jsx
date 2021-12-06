import React from 'react'
import I18n from 'i18n-js'

const emphasize = (name, isEmphasized) => isEmphasized ? <em>{name}</em> : name

const arrow = <span>{" "}&rarr;{" "}</span>

export const Breadcrumbs = ({ step }) => (
  <div className="db-bot__infotext--setup">
    <span className="db-breadcrumbs">
      {emphasize('Type', step === 0)}
      {arrow}
      {emphasize(I18n.t('bots.setup.step_exchange'), step === 1)}
      {arrow}
      {emphasize(I18n.t('bots.setup.step_api_key'), step === 2)}
      {arrow}
      {emphasize(I18n.t('bots.setup.step_schedule'), step === 3)}
    </span>
  </div>
)
