import React, { useState, useEffect, memo } from 'react';
import moment from 'moment';
import { connect } from 'react-redux';
import { useInterval } from '../utils/interval';
import { formatDuration } from '../utils/time';
import { Spinner } from './Spinner';
import API from '../lib/API';
import { StartButton, StopButton, RemoveButton } from './buttons'
import { Timer } from './Timer';
import { ProgressBar } from './ProgressBar';
import { isNotEmpty } from '../utils/array';
import {
  reloadBot,
  startBot,
  stopBot,
  removeBot,
  editBot,
  openBot,
} from '../bot_actions'

const BotTemplate = ({
  bot,
  errors = [],
  isPending,
  handleStart,
  handleStop,
  handleRemove,
  handleClick,
  handleEdit,
  reload,
  open
}) => {
  const { id, settings, status, exchangeName, nextTransactionTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}

  const [price, setPrice] = useState(settings.price);
  const [interval, setInterval] = useState(settings.interval);

  const description = `${settings.type} for ${settings.price}${settings.currency}/${settings.interval} on ${exchangeName}`
  const colorClass = settings.type == 'buy' ? 'success' : 'danger'
  const botOpenClass = open ? 'db-bot--active' : 'db-bot--collapsed'
  const working = status == 'working'

  const disableSubmit = price.trim() == ''

  const _handleSubmit = (evt) => {
    if (disableSubmit) return undefined

    const botParams = { interval, id: bot.id, price: price.trim() }
    handleEdit(botParams)
  }

  // Shows the first (major) error
  const Errors = ({ data }) => (
    <div className="db-bot__infotext__right">
      { data[0] }
    </div>
  )

  // useEffect(() => {}, [JSON.stringify(bot)])

  const handleReload = (bot, callback) => {
    reload(bot)
    callback()
  }

  return (
    <div onClick={() => handleClick(id)} className={`db-bots__item db-bot db-bot--dca db-bot--pick-exchange db-bot--running ${botOpenClass}`}>
      <div className="db-bot__header">
        { working ? <StopButton onClick={() => handleStop(id)} /> : <StartButton onClick={() => _handleSubmit(id)}/> }
        <div className={`db-bot__infotext text-${colorClass}`}>
          <div className="db-bot__infotext__left">
            <span className="d-none d-sm-inline">{ exchangeName }:</span>BTC{settings.currency}
          </div>
          { working && nextTransactionTimestamp && <Timer bot={bot} callback={reload} isPending={isPending}/> }
          { !working && isNotEmpty(errors) && <Errors data={errors} /> }
          <ProgressBar bot={bot} />
        </div>
      </div>

      <div className="row db-bot__form">
        <form className="form-inline mx-4">
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
          <div className="form-group mr-2">
            <select
              className="form-control"
              disabled={true}
            >
              <option value="BTC">BTC</option>
              <option value="ETH">ETH</option>
              <option value="LTC">LTC</option>
              <option value="XMR">XMR</option>
            </select>
          </div>
          <div className="form-group mr-2">for</div>
          <div className="form-group mr-2">
            <input
              type="text"
              value={price}
              onChange={e => setPrice(e.target.value)}
              className="form-control"
              disabled={working ? true : false}
            />
          </div>
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
              <option value="hour">Hour</option>
              <option value="day">Day</option>
              <option value="week">Week</option>
              <option value="month">Month</option>
            </select>
          </div>
        </form>
      </div>
      <RemoveButton onClick={() => handleRemove(id)} disabled={working}/>
    </div>
  )
}

const isCurrentBotPending = (bot, pending) => {
  if(!bot) {
    return false;
  }

  return pending[bot.id];
}


const mapStateToProps = (state) => {
  const currentBot = state.bots.find(bot => bot.id === state.currentBotId)
  return {
    bots: state.bots,
    currentBot: currentBot,
    isPending: isCurrentBotPending(currentBot, state.isPending)
  }
}

const mapDispatchToProps = ({
  reload: reloadBot,
  handleStart: startBot,
  handleStop: stopBot,
  handleRemove: removeBot,
  handleEdit: editBot,
  handleClick: openBot,
})
export const Bot = connect(mapStateToProps, mapDispatchToProps)(BotTemplate)
