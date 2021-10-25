import React from 'react'
import { ExchangeButton } from '../buttons'
import { Breadcrumbs } from './Breadcrumbs'
import { Progressbar } from './Progressbar'

export const PickBotType = ({ handleSubmit }) => {
  return (
    <div className="db-bots__item db-bot db-bot--pick-exchange db-bot--active">
      <div className="db-bot__header">
        <Breadcrumbs step={0} />
        <div className="db-bot__infotext" />
      </div>
      <Progressbar value={0} />
      <div className="row db-bot__types">
        <div
          className="db-bot__types__item db-bot__types__item--dca"
          onClick={() => handleSubmit('trading')}
        >
          Dollar-Cost Averaging
        </div>
        <div
          className="db-bot__types__item db-bot__types__item--aw"
          onClick={() => handleSubmit('withdrawal')}
        >
          Automatic Withdrawal
        </div>
      </div>
    </div>
  )
}
