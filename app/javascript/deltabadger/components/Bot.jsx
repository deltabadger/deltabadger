import React, { useState, useEffect, memo } from 'react';
import moment from 'moment';
import { useInterval } from '../utils/interval';
import { formatDuration } from '../utils/time';
import { Spinner } from './Spinner';
import API from '../lib/API';

export const Bot = props => {
  const [bot, setBot] = useState(props.bot)

  const { id, settings, status, exchangeName, nextTransactionTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}
  const { handleStart, handleStop, handleRemove, handleClick, open, reload } = props

  const description = `${settings.type} for ${settings.price}${settings.currency}/${settings.interval} on ${exchangeName}`
  const colorClass = settings.type == 'buy' ? 'success' : 'danger'
  const botOpenClass = open ? 'db-bot--active' : 'db-bot--collapsed'
  const working = status == 'working'

  const _handleStart = (id) => {
    API.startBot(id).then(({data: bot}) => {
      setBot(bot)
      handleStart(bot.id)
    })
  }
  const _handleStop = (id) => {
    API.stopBot(id).then(({data: bot}) => {
      setBot(bot)
      handleStop(bot.id)
    })
  }

  const StartButton = () => (
    <div onClick={() => _handleStart(id)} className="btn btn-success"><span>Start</span> <i className="material-icons">play_arrow</i></div>
  )
  const StopButton = () => (
    <div onClick={() => _handleStop(id)} className="btn btn-outline-primary"><span>Pause</span> <i className="material-icons">pause</i></div>
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

  const ProgressBar = () => {
    const [progress, setProgress] = useState(0)

    if (working) {
      useInterval(() => {
        const lastTransactionTimestamp = ([...props.bot.transactions].pop() || {}).created_at_timestamp
        const now  = new moment()
        const nowTimestamp = now.unix()
        const calc = ((nowTimestamp - lastTransactionTimestamp)/(nextTransactionTimestamp - lastTransactionTimestamp)) * 100

        setProgress(calc)
      }, 1000);
    }

    return (
      <div className="progress progress--thin progress--bot-setup">
        <div className={`progress-bar bg-${colorClass}`} role="progressbar" style={{width: `${progress}%`, ariaValuenow: progress.toString(), ariaValuemin: "0", ariaValuemax: "100"}}></div>
      </div>
    )
  }

  const Timer = memo(() => {
    const [delay, setDelay] = useState(undefined)

    const calculateDelay = () => {
      const now = new moment()
      const date = nextTransactionTimestamp && new moment.unix(nextTransactionTimestamp)

      return nextTransactionTimestamp && moment.duration(date.diff(now))
    }

    useInterval(() => {
      const delay = calculateDelay()
      setDelay(delay)
    }, 1000);

    if (!delay || !formatDuration(delay) || !nextTransactionTimestamp) {
      return (<Spinner />)
    }

    return (
      <div className="db-bot__infotext__right">
        Next { settings.type } in { formatDuration(delay) }
      </div>
    )
  })

  return (
    <div onClick={() => handleClick(id)} className={`db-bots__item db-bot db-bot--dca db-bot--pick-exchange db-bot--running ${botOpenClass}`}>
      <div className="db-bot__header">
        { working ? <StopButton /> : <StartButton/> }
        <div className={`db-bot__infotext text-${colorClass}`}>
          <div className="db-bot__infotext__left">
            { exchangeName }:BTC{settings.currency}
          </div>
          { working && nextTransactionTimestamp && <Timer /> }
          <ProgressBar />
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
  )
}
