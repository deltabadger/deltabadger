import React, { useState } from 'react';
import moment from 'moment';

export const Bot = ({
  id,
  settings,
  status,
  exchangeName,
  nextTransactionTimestamp,
  handleStart,
  handleStop,
  handleClick,
  open
}) => {
  const description = `${settings.type} for ${settings.price}${settings.currency}/${settings.interval} on ${exchangeName}`

  const StartButton = () => (
    <div onClick={() => handleStart(id)} className="btn btn-success"><span>Start</span> <i className="fas fa-play"></i></div>
  )
  const StopButton = () => (
    <div onClick={() => handleStop(id)} className="btn btn-outline-primary"><span>Pause</span> <i className="fas fa-pause"></i></div>
  )

  const botOpenClass = open ? '' : 'db-bot--collapsed'

  const duration = moment(nextTransactionTimestamp, '%X').fromNow();

  const working = status == 'working'

  return (
    <div onClick={() => handleClick(id)} className={`db-bots__item db-bot db-bot--dca db-bot--pick-exchange db-bot--running ${botOpenClass}`}>
      <div className="db-bot__header">
        { working ? <StopButton /> : <StartButton/> }
        <div className="db-bot__infotext text-danger">
          <div className="db-bot__infotext__left">
            { exchangeName }
          </div>
          { working &&
              <div className="db-bot__infotext__right">
                Next { settings.type } {duration}
              </div>
          }
          <div className="progress progress--thin progress--bot-setup">
            <div className="progress-bar bg-danger" role="progressbar" style={{width: "10%", ariaValuenow: "25", ariaValuemin: "0", ariaValuemax: "100"}}></div>
          </div>
        </div>
      </div>

      <div className="row db-bot--dca__config-free">
        Price: {settings.price}<br />
        Currency: {settings.currency}<br />
        Type: {settings.type}
      </div>
    </div>
  )
}
