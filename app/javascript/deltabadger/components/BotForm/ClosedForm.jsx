import React from 'react'
import I18n from 'i18n-js'
import { splitTranslation } from "../helpers";

export const ClosedForm = ({ handleSubmit }) => (
  <div className="db-bots__item d-flex justify-content-center db-add-more-bots">
    {splitTranslation(I18n.t('bots.add_new_bot_html'))[0]}
    <button onClick={() => handleSubmit} className="btn btn-link">
      {splitTranslation(I18n.t('bots.add_new_bot_html'))[1]}
    </button>
    {splitTranslation(I18n.t('bots.add_new_bot_html'))[2]}
    <button onClick={handleSubmit} className="btn btn-link">
      {splitTranslation(I18n.t('bots.add_new_bot_html'))[3]}
    </button>
  </div>
)
