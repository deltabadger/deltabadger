import React from 'react';

export const Bot = ({ id, settings, status, exchangeName, handleStart, handleStop }) => {
  const description = `${settings.type} for ${settings.price}${settings.currency}/${settings.interval} on ${exchangeName}`

  const StartButton = () => (<button onClick={() => handleStart(id)}>Start</button>)
  const StopButton = () => (<button onClick={() => handleStop(id)}>Stop</button>)

  // return (
  //   <div>
  //     { description }
  //     { status == 'working' ? <StopButton /> : <StartButton/> }
  //   </div>
  // )

  return (
    <div className="db-bots__item db-bot db-bot--dca db-bot--pick-exchange db-bot--running db-bot--collapsed">
      <div className="db-bot__header">
        <div className="btn btn-outline-primary"><span>Pause</span> <i className="fas fa-pause"></i></div>
        <div className="db-bot__infotext text-danger">
          <div className="db-bot__infotext__left">
            Kraken:BTCEUR
          </div>
          <div className="db-bot__infotext__right">
            Next sell in 25:14:18
          </div>
          <div className="progress progress--thin progress--bot-setup">
            <div className="progress-bar bg-danger" role="progressbar" style={{width: "22%", ariaValuenow: "25", ariaValuemin: "0", ariaValuemax: "100"}}></div>
          </div>
        </div>
      </div>
    </div>
  )
}
