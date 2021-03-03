import React from 'react'
import I18n from 'i18n-js'

export const ClosedForm = ({ handleSubmit }) => (
  <div className="db-bots__item d-flex justify-content-center db-add-more-bots">
    <button onClick={handleSubmit} className="btn btn-link">
      {I18n.t('bots.add_new_bot')} +
    </button>
  </div>
)
