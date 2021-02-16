import 'lodash'
import React, { useState } from 'react';
import I18n from 'i18n-js'
import { connect } from 'react-redux';
import { startButtonType, StartButton, StartingButton, StopButton, RemoveButton } from './buttons'
import { Timer } from './Timer';
import { ProgressBar } from './ProgressBar';
import LimitOrderNotice from "./BotForm/LimitOrderNotice";
import { isNotEmpty } from '../utils/array';
import { shouldRename, renameSymbol} from "../utils/symbols";

import {
  reloadBot,
  stopBot,
  removeBot,
  editBot,
  openBot,
  clearErrors,
  fetchRestartParams
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
  fetchRestartParams,
  clearBotErrors,
  reload,
  open
}) => {
  const { id, settings, status, exchangeName, nextTransactionTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}

  const [type, setType] = useState(settings.order_type);
  const [price, setPrice] = useState(settings.price);
  const [percentage, setPercentage] = useState(settings.percentage);
  const [interval, setInterval] = useState(settings.interval);
  const [forceSmartIntervals, setForceSmartIntervals] = useState(settings.force_smart_intervals);

  const colorClass = settings.type === 'buy' ? 'success' : 'danger'
  const botOpenClass = open ? 'db-bot--active' : 'db-bot--collapsed'
  const isStarting = startingBotIds.includes(id);
  const working = status === 'working'

  const disableSubmit = price.trim() === ''

  const isLimitSelected = () => type === 'limit'

  const hasConfigurationChanged = () => {
    const newSettings= {
      order_type: type,
      interval,
      price: price.trim(),
      forceSmartIntervals,
      percentage: isLimitSelected() ? percentage && percentage.trim() : undefined
    }

    const oldSettings = {
      order_type: settings.order_type,
      interval: settings.interval,
      price: settings.price.trim(),
      forceSmartIntervals: settings.force_smart_intervals,
      percentage: settings.order_type === 'limit' ? percentage.trim() : undefined
    }

    return !_.isEqual(newSettings, oldSettings)
  }

  const getStartButtonType = () => {
    if (hasConfigurationChanged()) {
      return fetchRestartParams(bot.id).then((data) => {
        switch (data.restartType) {
          case startButtonType.MISSED:
            return {...data, restartType: startButtonType.CHANGED_MISSED}
          case startButtonType.ON_SCHEDULE:
            return {...data, restartType: startButtonType.CHANGED_ON_SCHEDULE}
          case startButtonType.FAILED:
            return {...data, restartType: startButtonType.FAILED}
        }
      })
    }

    return fetchRestartParams(bot.id)
  }

  const _handleSubmit = (continueSchedule = false, fixing_price = null) => {
    if (disableSubmit) return

    const botParams = {
      order_type: type,
      interval,
      id: bot.id,
      price: price.trim(),
      forceSmartIntervals,
      percentage: isLimitSelected() ? percentage && percentage.trim() : undefined
    }

    const continueParams = {
      price: fixing_price,
      continueSchedule
    }

    handleEdit(botParams, continueParams)
  }

  // Shows the first (major) error
  const Errors = ({ data }) => (
    <div className="db-bot__infotext__right">
      { data[0] }
    </div>
  )

  const isSellOffer = () => settings.type === 'sell'

  const baseName = shouldRename(exchangeName) ? renameSymbol(settings.base) : settings.base
  const quoteName = shouldRename(exchangeName) ? renameSymbol(settings.quote) : settings.quote

  const handleTypeChange = (e) => {
    setType(e.target.value)
    clearBotErrors(id)
  }

  return (
    <div onClick={() => handleClick(id)} className={`db-bots__item db-bot db-bot--dca db-bot--setup-finished ${botOpenClass}`}>
      <div className="db-bot__header">
        { isStarting && <StartingButton /> }
        { !isStarting && (working ? <StopButton onClick={() => handleStop(id)} /> :
            <StartButton settings={settings} getRestartType={getStartButtonType} onClickReset={_handleSubmit}/>)  }
        <div className={`db-bot__infotext text-${colorClass}`}>
          <div className="db-bot__infotext__left">
            <span className="d-none d-sm-inline">{ exchangeName }:</span>{baseName}{quoteName}
          </div>
          { working && nextTransactionTimestamp && <Timer bot={bot} callback={reload} /> }
          { !working && isNotEmpty(errors) && <Errors data={errors} /> }
        </div>
      </div>

      <ProgressBar bot={bot} />

      <div className="db-bot__form">
        <form>
          <div className="form-inline mx-4">
            <div className="form-group mr-2">
              <select
                value={type}
                onChange={handleTypeChange}
                className="form-control db-select--buy-sell"
                disabled={working}
              >
                {isSellOffer() ? <>
                    <option value="market">{I18n.t('bots.sell')}</option>
                    <option value="limit" disabled={!showLimitOrders}>{I18n.t('bots.limit_sell')}</option>
                  </>
                  : <>
                    <option value="market">{I18n.t('bots.buy')}</option>
                    <option value="limit" disabled={!showLimitOrders}>{I18n.t('bots.limit_buy')}</option>
                  </>
                }
              </select>
            </div>
            <div className="form-group mr-2"> {baseName} {I18n.t('bots.for')}</div>
            <div className="form-group mr-2">
              <input
                type="tel"
                min="1"
                value={price}
                onChange={e => setPrice(e.target.value)}
                className="form-control db-input--dca-amount"
                disabled={working}
              />
            </div>
            <div className="form-group mr-2"> {quoteName} /</div>
            <div className="form-group mr-2">
              <select
                value={interval}
                className="form-control"
                onChange={e => setInterval(e.target.value)}
                disabled={working}
              >
                <option value="hour">{I18n.t('bots.hour')}</option>
                <option value="day">{I18n.t('bots.day')}</option>
                <option value="week">{I18n.t('bots.week')}</option>
                <option value="month">{I18n.t('bots.month')}</option>
              </select>
            </div>
          </div>
          <label className="form-inline mx-4 mt-4 mb-0">
            <input
              type="checkbox"
              checked={forceSmartIntervals}
              disabled={working}
              onChange={() => setForceSmartIntervals(!forceSmartIntervals)}
              className="mr-2" />
            <span disabled={working}>{I18n.t('bots.force_smart_intervals')}</span>
          </label>
        </form>
        {isLimitSelected() &&
        <span className="db-limit-bot-modifier">
          { isSellOffer() ? 'Sell' : 'Buy' } <input
            type="text"
            min="0"
            step="0.1"
            value={percentage}
            className="form-control"
            onChange={e => setPercentage(e.target.value)}
            placeholder="0"
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
  clearBotErrors: clearErrors,
  fetchRestartParams: fetchRestartParams
})
export const Bot = connect(mapStateToProps, mapDispatchToProps)(BotTemplate)
