import React, { useState, useEffect, memo } from 'react';
import moment from 'moment';
import { useInterval } from '../utils/interval';
import { formatDuration } from '../utils/time';
import { Spinner } from './Spinner';
import API from '../lib/API';
import { StartButton, StopButton, RemoveButton } from './buttons'

export const Bot = props => {
  const [bot, setBot] = useState(props.bot)

  const { handleStart, handleStop, handleRemove, handleClick, open } = props
  const { id, settings, status, exchangeName, nextTransactionTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}

  const [price, setPrice] = useState(settings.price);
  const [interval, setInterval] = useState(settings.interval);

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

  const disableSubmit = price.trim() == ''

  const _handleSubmit = (evt) => {
    if (disableSubmit) return undefined

    const botParams = { interval, id: bot.id, price: price.trim() }
    API.updateBot(botParams).then(({data: bot}) => {
      _handleStart(bot.id)
    })
  }

  const ProgressBar = ({bot}) => {
    const [progress, setProgress] = useState(0)

    if (working) {
      useInterval(() => {
        const now  = new moment()
        const nowTimestamp = now.unix()
        const lastTransactionTimestamp = ([...bot.transactions].pop() || {}).created_at_timestamp
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

  const Timer = ({bot}) => {
    const [delay, setDelay] = useState(undefined)

    const calculateDelay = () => {
      const now = new moment()
      const date = bot.nextTransactionTimestamp && new moment.unix(bot.nextTransactionTimestamp)

      return bot.nextTransactionTimestamp && moment.duration(date.diff(now))
    }

    if (working) {
      useInterval(() => {

        const delay = calculateDelay()
        setDelay(delay)
      }, 1000);

      if (!delay || !bot.nextTransactionTimestamp) {
        return (<Spinner />)
      }

      return (
        <div className="db-bot__infotext__right">
          Next { settings.type } in { formatDuration(delay) }
        </div>
      )
    }
  }

  return (
    <div onClick={() => handleClick(id)} className={`db-bots__item db-bot db-bot--dca db-bot--pick-exchange db-bot--running ${botOpenClass}`}>
      <div className="db-bot__header">
        { working ? <StopButton onClick={() => _handleStop(id)} /> : <StartButton onClick={() => _handleSubmit(id)}/> }
        <div className={`db-bot__infotext text-${colorClass}`}>
          <div className="db-bot__infotext__left">
            { exchangeName }:BTC{settings.currency}
          </div>
          { working && nextTransactionTimestamp && <Timer bot={bot} /> }
          <ProgressBar bot={bot} />
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
            value={price}
            onChange={e => setPrice(e.target.value)}
            className="form-control mr-1"
            disabled={working ? true : false}
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
            value={interval}
            className="form-control"
            onChange={e => setInterval(e.target.value)}
            id="exampleFormControlSelect1"
            disabled={working ? true : false}
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
    <RemoveButton onClick={() => handleRemove(id)} />
  </div>
  )
}
