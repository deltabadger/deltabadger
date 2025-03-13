import React from 'react'
import I18n from 'i18n-js'

export const ClosedForm = ({ handleSubmit }) => (
  <div className="db-bots__item d-flex db-add-more-bots">
    <button onClick={() => handleSubmit()} className="button button--primary">
      <span className="d-none">{I18n.t('bots.add_new_bot')}</span>
      <i className="material-icons">add</i>
    </button>
  </div>
)
