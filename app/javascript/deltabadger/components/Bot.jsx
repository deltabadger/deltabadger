import React, { useState } from 'react';
import { connect } from 'react-redux';
import { StartButton, StartingButton, StopButton, RemoveButton } from './buttons'
import { Timer } from './Timer';
import { ProgressBar } from './ProgressBar';
import LimitOrderNotice from "./BotForm/LimitOrderNotice";
import { isNotEmpty } from '../utils/array';
import {
  reloadBot,
  stopBot,
  removeBot,
  editBot,
  openBot,
} from '../bot_actions'

const BotTemplate = ({
  showLimitOrders,
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

  const [type, setType] = useState(settings.order_type);
  const [price, setPrice] = useState(settings.price);
  const [percentage, setPercentage] = useState(settings.percentage);
  const [interval, setInterval] = useState(settings.interval);

  const colorClass = settings.type === 'buy' ? 'success' : 'danger'
  const botOpenClass = open ? 'db-bot--active' : 'db-bot--collapsed'
  const isStarting = startingBotIds.includes(id);
  const working = status === 'working'

  const disableSubmit = price.trim() === ''

  const isLimitSelected = () => type === 'limit'

  const _handleSubmit = () => {
    if (disableSubmit) return

    const botParams = {
      order_type: type,
      interval,
      id: bot.id,
      price: price.trim(),
      percentage: isLimitSelected() ? percentage && percentage.trim() : undefined
    }
    handleEdit(botParams)
  }

  // Shows the first (major) error
  const Errors = ({ data }) => (
    <div className="db-bot__infotext__right">
      { data[0] }
    </div>
  )

  const isSellOffer = () => settings.type === 'sell'

  return (
    <div onClick={() => handleClick(id)} className={`db-bots__item db-bot db-bot--dca db-bot--active db-bot--setup-finished ${botOpenClass}`}>
      <div className="db-bot__header">
        { isStarting && <StartingButton /> }
        { !isStarting && (working ? <StopButton onClick={() => handleStop(id)} /> : <StartButton onClick={_handleSubmit}/>) }
        <div className={`db-bot__infotext text-${colorClass}`}>
          <div className="db-bot__infotext__left">
            <span className="d-none d-sm-inline">{ exchangeName }:</span>{settings.base}{settings.quote}
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
              value={type}
              onChange={e => setType(e.target.value)}
              className="form-control db-select--buy-sell"
              id="exampleFormControlSelect1"
              disabled={working}
            >
              {isSellOffer() ? <>
                  <option value="market">Sell</option>
                  <option value="limit" disabled={!showLimitOrders}>Limit Sell</option>
                </>
                : <>
                  <option value="market">Buy</option>
                  <option value="limit" disabled={!showLimitOrders}>Limit Buy</option>
                </>
              }
            </select>
          </div>
          <div className="form-group mr-2"> {settings.base} for</div>
          <div className="form-group mr-2">
            <input
              type="text"
              min="1"
              value={price}
              onChange={e => setPrice(e.target.value)}
              className="form-control db-input--dca-amount"
              disabled={working}
            />
          </div>
          <div className="form-group mr-2"> {settings.quote} /</div>
          <div className="form-group mr-2">
            <select
              value={interval}
              className="form-control"
              onChange={e => setInterval(e.target.value)}
              id="exampleFormControlSelect1"
              disabled={working}
            >
              <option value="hour">Hour</option>
              <option value="day">Day</option>
              <option value="week">Week</option>
              <option value="month">Month</option>
            </select>
          </div>
        </form>
        {isLimitSelected() &&
        <span className="db-limit-bot-modifier">
          { isSellOffer() ? 'Sell' : 'Buy' } <input
            type="text"
            min="0"
            step="0.1"
            lang="en-150"
            value={percentage}
            className="form-control"
            onChange={e => setPercentage(e.target.value)}
            disabled={working}
        /> % { isSellOffer() ? 'above' : 'below'} the price.<sup>*</sup></span> }
      </div>
      {isLimitSelected() && <LimitOrderNotice />}
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
