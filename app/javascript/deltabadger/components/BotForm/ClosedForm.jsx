import React from 'react'
import I18n from 'i18n-js'

export const ClosedForm = ({ handleSubmit }) => (
  <div className="db-bots__item d-flex db-add-more-bots">
    <button onClick={() => handleSubmit()} className="sbutton sbutton--primary">
      <span className="d-none mr-3">{I18n.t('bots.add_new_bot')}</span>
      <i className="material-icons">add</i>
    </button>
  </div>
)
