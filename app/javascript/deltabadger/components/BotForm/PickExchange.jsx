import React from 'react'
import { ExchangeButton } from '../buttons'
import { Breadcrumbs } from './Breadcrumbs'
import { Progressbar } from './Progressbar'

export const PickExchage = ({ handleSubmit, exchanges }) => {
  return (
    <div className="db-bots__item db-bot db-bot--pick-exchange db-bot--active">
      <div className="db-bot__header">
        <Breadcrumbs step={0} />
        <div className="db-bot__infotext" />
      </div>
      <Progressbar value={0} />
      <div className="row db-bot__exchanges">
        {
          exchanges.map(e =>
            <ExchangeButton key={e.id} handleClick={handleSubmit} exchange={e} />
          )
        }
      </div>
    </div>
  )
}
