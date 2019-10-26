import React, { useState } from 'react';
import moment from 'moment';
import { BotDetails } from './BotDetails';

export const Bot = props => {
  const { id, settings, status, exchangeName, nextTransactionTimestamp } = props.bot
  const { handleStart, handleStop, handleRemove, handleClick, open } = props

  const description = `${settings.type} for ${settings.price}${settings.currency}/${settings.interval} on ${exchangeName}`

  const StartButton = () => (
    <div onClick={() => handleStart(id)} className="btn btn-success"><span>Start</span> <i className="material-icons">play_arrow</i></div>
  )
  const StopButton = () => (
    <div onClick={() => handleStop(id)} className="btn btn-outline-primary"><span>Pause</span> <i className="material-icons">pause</i></div>
  )

  const RemoveButton = () => (
    <div
      onClick={() => handleRemove(id)}
      className="btn btn-link btn--reset"
    >
    <i className="material-icons">sync</i>
      <span>Reset</span>
    </div>
  )

  const botOpenClass = open ? 'db-bot--active' : 'db-bot--collapsed'

  const duration = nextTransactionTimestamp && moment(nextTransactionTimestamp, '%X').fromNow();

  const working = status == 'working'
  const colorClass = settings.type == 'buy' ? 'success' : 'danger'


  return (
    <div>
      <div onClick={() => handleClick(id)} className={`db-bots__item db-bot db-bot--dca db-bot--pick-exchange db-bot--running ${botOpenClass}`}>
        <div className="db-bot__header">
          { working ? <StopButton /> : <StartButton/> }
          <div className={`db-bot__infotext text-${colorClass}`}>
            <div className="db-bot__infotext__left">
              { exchangeName }:BTC{settings.currency}
            </div>
            { working &&
                <div className="db-bot__infotext__right">
                  Next { settings.type } {duration}
                </div>
            }
            <div className="progress progress--thin progress--bot-setup">
              <div className={`progress-bar bg-${colorClass}`} role="progressbar" style={{width: "10%", ariaValuenow: "25", ariaValuemin: "0", ariaValuemax: "100"}}></div>
            </div>
          </div>
        </div>

        <div className="row db-bot--dca__config-free">
          <form className="form-inline">
            <div className="form-group mr-2">
              <select
                value={settings.type}
                className="form-control"
                id="exampleFormControlSelect1"
                disabled={true}
              >
                <option value="buy">Buy</option>
                <option value="sell">Sell</option>
              </select>
            </div>
            <div className="form-group mr-2">for</div>
            <input
              type="text"
              value={settings.price}
              className="form-control mr-1"
              disabled={true}
            />

          <div className="form-group mr-2">
            <select
              value={settings.currency}
              className="form-control"
              id="exampleFormControlSelect1"
              disabled={true}
            >
              <option value="">{settings.currency}</option>
            </select>
          </div>
          <div className="form-group mr-2">/</div>
          <div className="form-group mr-2">
            <select
              value={settings.interval}
              className="form-control"
              id="exampleFormControlSelect1"
              disabled={true}
            >
              <option value="minute">Minute</option>
              <option value="hour">Hour</option>
              <option value="day">Day</option>
              <option value="week">Week</option>
              <option value="month">Month</option>
            </select>
          </div>
        </form>
      </div>
      <RemoveButton />
    </div>
    { open && <BotDetails bot={props.bot} /> }
  </div>
  )
}
