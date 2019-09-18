import React from 'react'

const ExchangeButton = ({ handleClick, exchange }) => (
    <button onClick={ () => handleClick(exchange.id) }>
      <div className={`col-sm-6 col-md-4 db-bot__exchanges__item db-bot__exchanges__item--${exchange.name.toLowerCase()}`}></div>
    </button>
  )

export const PickExchage = ({ handleReset, handleSubmit, exchanges }) => {
  const CloseButton = () => (
    <div
      onClick={() => handleReset()}
      className="btn btn-link btn--reset"
    >
      Close<i className="fas fa-redo ml-1"></i>
    </div>
  )

  return (
    <div className="db-bots__item db-bot db-bot--pick-exchange">
      <div className="db-bot__header">
        <div className="db-bot__infotext db-bot__infotext--setup">Pick exchange (1 of 3)
          <div className="progress progress--thin progress--bot-setup">
            <div className="progress-bar" role="progressbar" style={{width: "20%", ariaValuenow: "20", ariaValuemin: "0", ariaValuemax: "100"}}></div>
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
      <CloseButton />
    </div>
  )
}
