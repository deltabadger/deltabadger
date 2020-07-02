import React, { useState } from 'react';
import { connect } from 'react-redux';
import { StartButton, StartingButton, StopButton, RemoveButton } from './buttons'
import { Timer } from './Timer';
import { ProgressBar } from './ProgressBar';
import { isNotEmpty } from '../utils/array';
import {
  reloadBot,
  stopBot,
  removeBot,
  editBot,
  openBot,
} from '../bot_actions'

const BotTemplate = ({
  bot,
  errors = [],
  startingBotIds,
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

  const colorClass = settings.type == 'buy' ? 'success' : 'danger'
  const botOpenClass = open ? 'db-bot--active' : 'db-bot--collapsed'
  const isStarting = startingBotIds.includes(id);
  const working = status == 'working'

  const disableSubmit = price.trim() == ''

  const _handleSubmit = () => {
    if (disableSubmit) return

    const botParams = { interval, id: bot.id, price: price.trim() }
    handleEdit(botParams)
  }

  // Shows the first (major) error
  const Errors = ({ data }) => (
    <div className="db-bot__infotext__right">
      { data[0] }
    </div>
  )

  return (
    <div onClick={() => handleClick(id)} className={`db-bots__item db-bot db-bot--dca db-bot--pick-exchange db-bot--running ${botOpenClass}`}>
      <div className="db-bot__header">
        { isStarting && <StartingButton /> }
        { !isStarting && (working ? <StopButton onClick={() => handleStop(id)} /> : <StartButton onClick={_handleSubmit}/>) }
        <div className={`db-bot__infotext text-${colorClass}`}>
          <div className="db-bot__infotext__left">
            <span className="d-none d-sm-inline">{ exchangeName }:</span>BTC{settings.currency}
          </div>
          { working && nextTransactionTimestamp && <Timer bot={bot} callback={reload} /> }
          { !working && isNotEmpty(errors) && <Errors data={errors} /> }
        </div>
      </div>

      <ProgressBar bot={bot} />

      <div className="db-bot__form">
        <form className="form-inline mx-4">
          <div className="form-group mr-2">
            <select
              value={settings.type}
              className="form-control db-select--buy-sell"
              id="exampleFormControlSelect1"
              disabled={true}
            >
              <option value="buy">Buy</option>
              <option value="sell">Sell</option>
              <option value="limit_buy" disabled>Limit Buy</option>
              <option value="limit_sell" disabled>Limit Sell</option>
            </select>
          </div>
          <div className="form-group mr-2">
            <select
              className="form-control"
              disabled={true}
            >
              <option value="BTC">BTC</option>
              <option value="ETH">ETH</option>
            </select>
          </div>
          <div className="form-group mr-2">for</div>
          <div className="form-group mr-2">
            <input
              type="text"
              min="1"
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
        {/* <span className="db-limit-bot-modifier">Buy <input type="text" min="0" step="0.1" lang="en-150" className="form-control" placeholder="0" /> % below the price.<sur>*</sur></span>*/}
      </div>
      {/* <small className="alert alert-warning db-alert--annotation">
        <sur>*</sur> The limit order bot is an experimental feature. The bot opens orders but does not track if they have been filled. Our backtesting showed that for buy orders, 2.0% below the price worked optimal in the past. <a href="#">Read more</a>
      </small>*/}
      <div className="db-bot__footer">
        <RemoveButton onClick={() => handleRemove(id)} disabled={working}/>
      </div>
    </div>
  )
}

const mapStateToProps = state => {
  return { startingBotIds: state.startingBotIds };
}

const mapDispatchToProps = ({
  reload: reloadBot,
  handleStop: stopBot,
  handleRemove: removeBot,
  handleEdit: editBot,
  handleClick: openBot,
})
export const Bot = connect(mapStateToProps, mapDispatchToProps)(BotTemplate)
