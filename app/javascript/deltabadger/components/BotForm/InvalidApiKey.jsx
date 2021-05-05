import React, { useState } from 'react'
import I18n from 'i18n-js'
import { RawHTML } from '../RawHtml'
import { Instructions } from './Instructions';
import { Breadcrumbs } from './Breadcrumbs'
import { Progressbar } from './Progressbar'

export const InvalidApiKey = ({
  pickedExchangeName,
  handleReset,
  handleTryAgain,
  handleRemove
}) => {
  const ResetButton = () => (
    <div
      onClick={() => handleReset()}
      className="btn btn-link btn--reset btn--reset-back"
    >
      <i className="material-icons-round">close</i>
      <span>{I18n.t('bots.setup.cancel')}</span>
    </div>
  )

  return (
    <div className="db-bots__item db-bot db-bot--get-apikey db-bot--active">
      <div className="db-bot__header">
        <Breadcrumbs step={4} />
      </div>
      <Progressbar value={33}/>
      <div className="db-bot__form db-bot__form--apikeys">
        <div>
          Wrong keys or insufficient permissions. You can check your permissions and try to validate keys again or you can add new API keys.
        </div>
        <div>
          <div onClick={() => handleTryAgain()} className="btn btn-outline-primary">
            Try again
          </div>
          <div onClick={() => handleRemove()} className="btn btn-success">
            Add new API keys
          </div>
        </div>
      </div>
      <Instructions exchangeName={pickedExchangeName} />
      <div className="db-bot__footer">
        <ResetButton />
      </div>
    </div>
  )
}
