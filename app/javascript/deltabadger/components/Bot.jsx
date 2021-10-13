import 'lodash'
import React, {useEffect, useState} from 'react';
import I18n from 'i18n-js'
import { connect } from 'react-redux';
import { startButtonType, StartButton, StartingButton, StopButton, RemoveButton, PendingButton } from './buttons'
import { Timer, FetchFromExchangeTimer } from './Timer';
import { ProgressBar } from './ProgressBar';
import LimitOrderNotice from "./BotForm/LimitOrderNotice";
import { isNotEmpty } from '../utils/array';
import {shouldRename, renameSymbol, renameCurrency} from "../utils/symbols";
import { RawHTML } from './RawHtml'
import { AddApiKey } from "./BotForm/AddApiKey";
import { removeInvalidApiKeys, splitTranslation } from "./helpers";

import {
  reloadBot,
  stopBot,
  removeBot,
  editBot,
  openBot,
  clearErrors,
  fetchRestartParams,
  getSmartIntervalsInfo,
  setShowSmartIntervalsInfo
} from '../bot_actions'
import API from "../lib/API";

const apiKeyStatus = {
  ADD: 'add_api_key',
  VALIDATING: 'validating_api_key',
  INVALID: 'invalid_api_key'
}

const BotTemplate = ({
  showLimitOrders,
  bot,
  errors = [],
  startingBotIds,
  handleStop,
  handleRemove,
  handleClick,
  handleEdit,
  fetchMinimums,
  fetchRestartParams,
  clearBotErrors,
  reload,
  reloadPage,
  open,
  fetchExchanges,
  exchanges,
  apiKeyTimeout
}) => {
  const { id, settings, status, exchangeName, exchangeId, nextResultFetchingTimestamp, nextTransactionTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}

  const [type, setType] = useState(settings.order_type);
  const [price, setPrice] = useState(settings.price);
  const [percentage, setPercentage] = useState(settings.percentage == null ? 0.0 : settings.percentage);
  const [interval, setInterval] = useState(settings.interval);
  const [forceSmartIntervals, setForceSmartIntervals] = useState(settings.force_smart_intervals);
  const [smartIntervalsValue, setSmartIntervalsValue] = useState(settings.smart_intervals_value == null ? "0" : settings.smart_intervals_value);
  const [minimumOrderParams, setMinimumOrderParams] = useState({});
  const [currencyOfMinimum, setCurrencyOfMinimum] = useState(settings.quote);
  const [priceRangeEnabled, setPriceRangeEnabled] = useState(settings.price_range_enabled)
  const [priceRange, setPriceRange] = useState({ low: settings.price_range[0].toString(), high: settings.price_range[1].toString() })
  const [apiKeyExists, setApiKeyExists] = useState(true)
  const [apiKeysState, setApiKeysState] = useState(apiKeyStatus["ADD"]);

  const isStarting = startingBotIds.includes(id);
  const working = status === 'working'
  const pending = status === 'pending'

  const colorClass = settings.type === 'buy' ? 'success' : 'danger'
  const botOpenClass = open ? 'db-bot--active' : 'db-bot--collapsed'
  const botRunningClass = working ? 'bot--running' : 'bot--stopped'

  const disableSubmit = price.trim() === ''

  const isLimitSelected = () => type === 'limit'

  const setLimitOrderCheckbox = () => {
    isLimitSelected() ? setType('market') : setType('limit')
  }

  const hasConfigurationChanged = () => {
    const newSettings= {
      order_type: type,
      interval,
      price: price.trim(),
      forceSmartIntervals,
      smartIntervalsValue,
      percentage: isLimitSelected() ? percentage && percentage.trim() : undefined
    }

    const oldSettings = {
      order_type: settings.order_type,
      interval: settings.interval,
      price: settings.price.trim(),
      forceSmartIntervals: settings.force_smart_intervals,
      smartIntervalsValue: settings.smartIntervalsValue,
      percentage: settings.order_type === 'limit' ? percentage && percentage.trim() : undefined
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
      percentage: isLimitSelected() ? percentage && percentage.trim() : undefined,
      smartIntervalsValue,
      priceRangeEnabled,
      priceRange
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
  const isLegacySell = () => settings.type === 'sell_old'

  const isLimitOrderDefinedInBase = (name) => ['Coinbase Pro', 'KuCoin'].includes(name)

  const baseName = shouldRename(exchangeName) ? renameSymbol(settings.base) : settings.base
  const quoteName = shouldRename(exchangeName) ? renameSymbol(settings.quote) : settings.quote

  const handleTypeChange = (e) => {
    setType(e.target.value)
    clearBotErrors(id)
  }

  const newSettings = () => {
    const out = {
      price: price.trim(),
      forceSmartIntervals: forceSmartIntervals
    }

    return out
  }

  const getBotParams = () => {
    return {
      type,
      exchangeName: exchangeName,
      base: settings.base,
      quote: settings.quote,
      interval: settings.interval,
      forceSmartIntervals,
      smartIntervalsValue,
      price: price.trim(),
      botType: 'free',
    }
  }

  const getMinimumOrderParams = (data) => {
    return {
      value: data.data.minimum,
      currency: data.data.side === 'base' ? renameCurrency(settings.base, exchangeName) : renameCurrency(settings.quote, exchangeName),
      showQuote: data.data.side === 'base',
      quoteValue: data.data.minimumQuote
    }
  }

  useEffect(() => {
    async function fetchSmartIntervalsInfo()  {
      const data = await fetchMinimums(getBotParams())
      if (isLimitOrderDefinedInBase(exchangeName) && isLimitSelected()) {
        data.data.minimum = data.data.minimum_limit
        data.data.side = 'base'
      }

      const minimum = data.data.minimum
      const currency = data.data.side === 'base' ? renameCurrency(settings.base, exchangeName) : renameCurrency(settings.quote, exchangeName)

      setMinimumOrderParams(getMinimumOrderParams(data))
      if (smartIntervalsValue === "0") {
        setSmartIntervalsValue(minimum.toString())
      }
      setCurrencyOfMinimum(currency)
    }

    fetchSmartIntervalsInfo()
  }, [type]);

  const validateSmartIntervalsValue = () => {
    if (isNaN(smartIntervalsValue) || smartIntervalsValue < minimumOrderParams.value){
      setSmartIntervalsValue(minimumOrderParams.value)
    }
  }

  const validatePercentage = () => {
    if (isNaN(percentage) || percentage < 0){
      setPercentage(0)
    }
  }

  const getSmartIntervalsDisclaimer = () => {
    if (minimumOrderParams.showQuote) {
      return I18n.t('bots.smart_intervals_disclaimer', {exchange: exchangeName, currency: currencyOfMinimum, minimum: minimumOrderParams.value})
    } else {
      return I18n.t('bots.smart_intervals_disclaimer_quote', {currency: currencyOfMinimum, minimum: minimumOrderParams.value});
    }
  }

  const keyOwned = (status) => status === 'correct'
  const keyPending = (status) => status === 'pending'
  const keyInvalid = (status) => status === 'incorrect'

  const keyExists = () => {
    // we have to assume that the key exists to fix unnecessary form rendering
    const exchange = exchanges.find(e => exchangeId === e.id) || {trading_key_status: true, withdrawal_key_status: true}
    const keyStatus = exchange.trading_key_status
    setApiKeyExists(keyOwned(keyStatus))

    if (keyOwned(keyStatus)) {
      clearTimeout(apiKeyTimeout)

    } else if (keyPending(keyStatus)) {
      setApiKeysState(apiKeyStatus["VALIDATING"])
      clearTimeout(apiKeyTimeout)
      apiKeyTimeout = setTimeout(() => fetchExchanges(), 3000)

    } else if (keyInvalid(keyStatus)) {
      clearTimeout(apiKeyTimeout)
      setApiKeysState(apiKeyStatus["INVALID"])
    }
  }

  useEffect(() => {
    keyExists()
  }, [exchanges]);

  const addApiKeyHandler = (key, secret, passphrase, germanAgreement) => {
    setApiKeysState(apiKeyStatus["VALIDATING"])
    API.createApiKey({ key, secret, passphrase, germanAgreement, exchangeId: exchangeId }).then(response => {
      fetchExchanges()
    }).catch(() => {
      setApiKeysState(apiKeyStatus["INVALID"])
    })
  }

  return (
    <div onClick={() => handleClick(id)} className={`db-bots__item db-bot db-bot--dca db-bot--setup-finished ${botOpenClass} ${botRunningClass}`}>
      { apiKeyExists &&
        <div className="db-bot__header">
          {isStarting && <StartingButton/>}
          {(!isStarting && working) && <StopButton onClick={() => handleStop(id)}/>}
          {(!isStarting && pending) && <PendingButton/>}
          {(!isStarting && !working && !pending) &&
          <StartButton settings={settings} getRestartType={getStartButtonType} onClickReset={_handleSubmit}
                       setShowInfo={setShowSmartIntervalsInfo} exchangeName={exchangeName} newSettings={newSettings()}/>}
          <div className={`db-bot__infotext text-${colorClass}`}>
            <div className="db-bot__infotext__left">
              {exchangeName}:{baseName}{quoteName}
            </div>
            {pending && nextResultFetchingTimestamp && <FetchFromExchangeTimer bot={bot} callback={reload}/>}
            {working && nextTransactionTimestamp && <Timer bot={bot} callback={reload}/>}
            {!working && isNotEmpty(errors) && <Errors data={errors}/>}
          </div>
        </div>
      }

      <ProgressBar bot={bot} />

      { !apiKeyExists &&
        <AddApiKey
          pickedExchangeName={exchangeName}
          handleReset={null}
          handleSubmit={addApiKeyHandler}
          handleRemove={() => removeInvalidApiKeys(exchangeId)}
          status={apiKeysState}
          botView={{baseName, quoteName}}
        />
      }

      { apiKeyExists &&
        <div className="db-bot__form">
        <form>
          <div className="form-inline db-bot__form__schedule">
            <div className="form-group mr-2">
              <select
                value={type}
                onChange={handleTypeChange}
                className="bot-input bot-input--select bot-input--order-type bot-input--paper-bg"
                disabled={working}
              >
                {isSellOffer() ? <>
                    <option value="market">{I18n.t('bots.sell')}</option>
                    <option value="limit" disabled={!showLimitOrders}>{I18n.t('bots.limit_sell')}</option>
                  </>
                  : <>
                    {isLegacySell() ?<>
                          <option value="market">{I18n.t('bots.sell')}</option>
                          <option value="limit" disabled={!showLimitOrders}>{I18n.t('bots.limit_sell')}</option>
                        </> :
                        <>
                          <option value="market">{I18n.t('bots.buy')}</option>
                          <option value="limit" disabled={!showLimitOrders}>{I18n.t('bots.limit_buy')}</option>
                        </>}
                  </>
                }
              </select>
            </div>
            {isSellOffer()?
                <>
                  <div className="form-group mr-2">
                    <input
                        type="text"
                        size={(price.length > 0) ? price.length : 3}
                        value={price}
                        className="bot-input bot-input--sizable bot-input--paper-bg"
                        onChange={e => setPrice(e.target.value)}
                        disabled={working}
                    />
                  </div>
                  <div className="form-group mr-2"> {baseName} {I18n.t('bots.for')}</div>
                </>
                :
                <>
                  <div className="form-group mr-2"> {baseName} {I18n.t('bots.for')}</div>
                  <div className="form-group mr-2">
                    <input
                        type="text"
                        size={(price.length > 0) ? price.length : 3}
                        value={price}
                        className="bot-input bot-input--sizable bot-input--paper-bg"
                        onChange={e => setPrice(e.target.value)}
                        disabled={working}
                    />
                  </div>
                </>

            }
            <div className="form-group mr-2"> {quoteName} /</div>
            <div className="form-group">
              <select
                value={interval}
                className="bot-input bot-input--select bot-input--interval  bot-input--paper-bg"
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

          <label
            className="alert alert-primary"
            disabled={!forceSmartIntervals}
          >
            <input
              type="checkbox"
              className="hide-when-running"
              checked={forceSmartIntervals}
              onChange={() => setForceSmartIntervals(!forceSmartIntervals)}
              disabled={working}
            />
            <div>
              <RawHTML tag="span">{splitTranslation(I18n.t('bots.force_smart_intervals_html', {currency: currencyOfMinimum}))[0]}</RawHTML>
              <input
                type="text"
                className="bot-input bot-input--sizable"
                value={smartIntervalsValue}
                size={smartIntervalsValue.length > 0 ? smartIntervalsValue.length : 3}
                onChange={e => setSmartIntervalsValue(e.target.value)}
                onBlur={validateSmartIntervalsValue}
                disabled={working}
              />
              <RawHTML tag="span">{splitTranslation(I18n.t('bots.force_smart_intervals_html', {currency: currencyOfMinimum}))[1]}</RawHTML>

              <small className="hide-when-running hide-when-disabled">
                <div>
                  <sup>*</sup>{getSmartIntervalsDisclaimer()}
                </div>
              </small>
            </div>

          </label>

          <label
            className="alert alert-primary"
            disabled={!showLimitOrders || !isLimitSelected()}
          >
            <input
              type="checkbox"
              className="hide-when-running"
              checked={isLimitSelected()}
              onChange={setLimitOrderCheckbox}
              disabled={working || !showLimitOrders}
            />
            <div>
              {isSellOffer() ? I18n.t('bots.sell') : (isLegacySell() ? I18n.t('bots.sell') : I18n.t('bots.buy'))} <input
              type="text"
              value={percentage}
              size={(percentage.length > 0) ? percentage.length : 1}
              className="bot-input bot-input--sizable"
              onChange={e => setPercentage(e.target.value)}
              onBlur={validatePercentage}
              disabled={working || !showLimitOrders || !isLimitSelected()}
            /> % {isSellOffer() ? I18n.t('bots.above') : (isLegacySell() ? I18n.t('bots.above') :I18n.t('bots.below'))} {I18n.t('bots.price')}.<sup
              className="hide-when-running">*</sup>

              { isLimitSelected() && <small className="hide-when-running"><LimitOrderNotice/></small> }
              { !showLimitOrders && <a href={`/${document.body.dataset.locale}/upgrade`} className="bot input bot-input--hodler-only--before">Hodler only</a> }
            </div>

          </label>


          <label
            className="alert alert-primary"
            disabled={!showLimitOrders || !priceRangeEnabled}
          >
            <input
              type="checkbox"
              className="hide-when-running"
              checked={priceRangeEnabled}
              onChange={() => setPriceRangeEnabled(!priceRangeEnabled)}
              disabled={working || !showLimitOrders}
            />
            <div>
              <RawHTML tag="span">{splitTranslation(I18n.t((isLegacySell() || isSellOffer()) ? 'bots.price_range_sell_html' :'bots.price_range_buy_html', {currency: settings.quote}))[0]}</RawHTML>
              <input
                type="text"
                className="bot-input bot-input--sizable"
                value={priceRange.low}
                onChange={e => setPriceRange({low: e.target.value, high: priceRange.high})}
                disabled={working || !showLimitOrders}
                size={Math.max(priceRange.low.length, 1)}
              />

              <RawHTML tag="span">{splitTranslation(I18n.t((isLegacySell() || isSellOffer()) ? 'bots.price_range_sell_html' :'bots.price_range_buy_html', {currency: settings.quote}))[1]}</RawHTML>
              <input
                type="text"
                className="bot-input bot-input--sizable"
                value={priceRange.high}
                onChange={e => setPriceRange({low: priceRange.low, high: e.target.value})}
                disabled={working || !showLimitOrders}
                size={ Math.max(priceRange.high.length, 1) }
              />
              <RawHTML tag="span">{splitTranslation(I18n.t((isLegacySell() || isSellOffer()) ? 'bots.price_range_sell_html' :'bots.price_range_buy_html', {currency: settings.quote}))[2]}</RawHTML>
              { !showLimitOrders && <a href={`/${document.body.dataset.locale}/upgrade`} className="bot input bot-input--hodler-only--before">Hodler only</a> }
            </div>
          </label>

        </form>

      </div>

      }
      <div className="db-bot__footer" hidden={working}>
        <RemoveButton onClick={() => { handleRemove(id).then(() => reloadPage()) }} disabled={working}/>
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
  fetchMinimums: getSmartIntervalsInfo,
  handleClick: openBot,
  clearBotErrors: clearErrors,
  fetchRestartParams: fetchRestartParams
})
export const Bot = connect(mapStateToProps, mapDispatchToProps)(BotTemplate)
