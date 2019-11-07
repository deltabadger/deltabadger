import React from 'react'
import { ExchangeButton, CloseButton } from '../buttons';

export const PickExchage = ({ handleReset, handleSubmit, exchanges }) => {
  return (
    <div className="db-bots__item db-bot db-bot--pick-exchange">
      <div className="db-bot__header">
        <div className="db-bot__infotext db-bot__infotext--setup">Pick exchange
          <div className="progress progress--thin progress--bot-setup">
            <div className="progress-bar" role="progressbar" style={{width: "0%", ariaValuenow: "0", ariaValuemin: "0", ariaValuemax: "100"}}></div>
          </div>
        </div>

      </div>
      <div className="row db-bot__exchanges">
        {
          exchanges.map(e =>
            <ExchangeButton key={e.id} handleClick={handleSubmit} exchange={e} />
          )
        }
      </div>
      <CloseButton onClick={handleReset} />
    </div>
  )
}
