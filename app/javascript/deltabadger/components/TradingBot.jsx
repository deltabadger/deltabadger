import 'lodash'
import React, {useEffect, useState} from 'react';
import I18n from 'i18n-js'
import { connect } from 'react-redux';
import { startButtonType, StartButton, StartingButton, StopButton, RemoveButton, PendingButton } from './buttons'
import { Timer, FetchFromExchangeTimer } from './Timer';
import { ProgressBar } from './ProgressBar';
import LimitOrderNotice from "./BotForm/LimitOrderNotice";
import { isNotEmpty } from '../utils/array';
import {shouldRename, renameSymbol, renameCurrency, shouldShowSubaccounts} from "../utils/symbols";
import { RawHTML } from './RawHtml'
import { AddApiKey } from "./BotForm/AddApiKey";
import { removeInvalidApiKeys, splitTranslation } from "./helpers";

import {
  reloadBot,
  stopBot,
  removeBot,
  editTradingBot,
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
  tileMode,
  onClick,
  buttonClickHandler,
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
  const defaultSettings = {
    order_type: 'market',
    price: "",
    percentage: 0.0,
    interval: "hour",
    force_smart_intervals: false,
    smart_intervals_value: "0",
    quote: "",
    price_range_enabled: false,
    price_range: [0, 0],
    use_subaccount: false,
    selected_subaccount: '',
    type: 'buy'
  };

  const { id, settings = defaultSettings, status, exchangeName, exchangeId, nextResultFetchingTimestamp, nextTransactionTimestamp } = bot || { settings: defaultSettings };

  const [type, setType] = useState(settings.order_type || defaultSettings.order_type);
  const [price, setPrice] = useState(settings.price || defaultSettings.price);
  const [percentage, setPercentage] = useState(settings.percentage !== undefined ? settings.percentage : defaultSettings.percentage);
  const [interval, setInterval] = useState(settings.interval || defaultSettings.interval);
  const [forceSmartIntervals, setForceSmartIntervals] = useState(settings.force_smart_intervals !== undefined ? settings.force_smart_intervals : defaultSettings.force_smart_intervals);
  const [smartIntervalsValue, setSmartIntervalsValue] = useState(settings.smart_intervals_value !== undefined ? settings.smart_intervals_value : defaultSettings.smart_intervals_value);
  const [minimumOrderParams, setMinimumOrderParams] = useState({});
  const [currencyOfMinimum, setCurrencyOfMinimum] = useState(settings.quote || defaultSettings.quote);
  const [priceRangeEnabled, setPriceRangeEnabled] = useState(settings.price_range_enabled !== undefined ? settings.price_range_enabled : defaultSettings.price_range_enabled);
  const [priceRange, setPriceRange] = useState({ 
    low: (settings.price_range && settings.price_range[0] !== undefined ? settings.price_range[0] : defaultSettings.price_range[0]).toString(), 
    high: (settings.price_range && settings.price_range[1] !== undefined ? settings.price_range[1] : defaultSettings.price_range[1]).toString() 
  });
  const [apiKeyExists, setApiKeyExists] = useState(true);
  const [apiKeysState, setApiKeysState] = useState(apiKeyStatus["ADD"]);
  const [useSubaccount,setUseSubaccounts] = useState(settings.use_subaccount !== undefined ? settings.use_subaccount : defaultSettings.use_subaccount);
  const [selectedSubaccount, setSelectedSubaccount] = useState(settings.selected_subaccount || defaultSettings.selected_subaccount);
  const [subaccountsList, setSubaccountsList] = useState(['']);

  const isStarting = startingBotIds.includes(id);
  const working = status === 'working'
  const pending = status === 'pending'

  const colorClass = settings.type === 'buy' ? 'success' : 'danger'
  const botOpenClass = open ? 'db-bot--active' : 'db-bot--collapsed'
  const botRunningClass = working ? 'bot--running' : 'bot--stopped'

  const disableSubmit = price.trim() === ''

  const isLimitSelected = () => type === 'limit'

  const [showSubaccounts,setShowSubaccounts] = useState(false)

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
      percentage: isLimitSelected() ? percentage && percentage.trim() : undefined,
      useSubaccount,
      selectedSubaccount
    }

    const oldSettings = {
      order_type: settings.order_type,
      interval: settings.interval,
      price: settings.price.trim(),
      forceSmartIntervals: settings.force_smart_intervals,
      smartIntervalsValue: settings.smart_intervals_value,
      percentage: settings.order_type === 'limit' ? percentage && percentage.trim() : undefined,
      useSubaccount: settings.use_subaccount,
      selectedSubaccount: settings.selected_subaccount
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
      botType: 'trading',
      order_type: type,
      interval,
      id: bot.id,
      price: price.trim(),
      forceSmartIntervals,
      percentage: isLimitSelected() ? percentage && percentage.trim() : undefined,
      smartIntervalsValue,
      priceRangeEnabled,
      priceRange,
      useSubaccount,
      selectedSubaccount
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
      botType: 'trading',
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
  const setSubaccounts = async () => {
    await API.getSubaccounts(exchangeId).then(data => {
      setSubaccountsList(data.data['subaccounts']);
      setShowSubaccounts(data.data['subaccounts'].length > 0 && shouldShowSubaccounts(exchangeName));
      setSelectedSubaccount(settings.use_subaccount ? settings.selected_subaccount : (data.data['subaccounts'].length > 0 ? data.data['subaccounts'][0] : ''));
    })
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

      await setSubaccounts()
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
    const exchange = exchanges.find(e => exchangeId === e.id) || {trading_key_status: true, withdrawal_key_status: true, webhook_key_status: true};
    const keyStatus = exchange.trading_key_status;
    setApiKeyExists(keyOwned(keyStatus));

    if (keyOwned(keyStatus)) {
      // No need for timeout
    } else if (keyPending(keyStatus)) {
      setApiKeysState(apiKeyStatus["VALIDATING"]);
    } else if (keyInvalid(keyStatus)) {
      setApiKeysState(apiKeyStatus["INVALID"]);
    }
    
    return keyStatus;
  };

  // Move useEffect outside of keyExists
  useEffect(() => {
    const keyStatus = keyExists();
    let timeoutId;

    if (keyPending(keyStatus)) {
      timeoutId = setTimeout(() => fetchExchanges(), 3000);
    }

    // Cleanup function
    return () => {
      if (timeoutId) {
        clearTimeout(timeoutId);
      }
    };
  }, [exchanges, fetchExchanges]);

  const addApiKeyHandler = (key, secret, passphrase, germanAgreement) => {
    setApiKeysState(apiKeyStatus["VALIDATING"])
    API.createApiKey({ key, secret, passphrase, germanAgreement, exchangeId: exchangeId }).then(response => {
      fetchExchanges()
    }).catch(() => {
      setApiKeysState(apiKeyStatus["INVALID"])
    })
  }

  const formatNumber = (value) => {
    const num = parseFloat(value);
    return isNaN(num) ? '0.00' : num.toFixed(2);
  };

  // Add this for debugging
  useEffect(() => {
    if (tileMode) {
      console.log('Bot stats:', bot.stats);
    }
  }, [bot.stats]);

  // Add useEffect to update values when bot changes
  useEffect(() => {
    if (bot) {
      const { settings } = bot;
      setType(settings.order_type || defaultSettings.order_type);
      setPrice(settings.price || defaultSettings.price);
      setPercentage(settings.percentage !== undefined ? settings.percentage : defaultSettings.percentage);
      setInterval(settings.interval || defaultSettings.interval);
      setForceSmartIntervals(settings.force_smart_intervals !== undefined ? settings.force_smart_intervals : defaultSettings.force_smart_intervals);
      setSmartIntervalsValue(settings.smart_intervals_value !== undefined ? settings.smart_intervals_value : defaultSettings.smart_intervals_value);
      setCurrencyOfMinimum(settings.quote || defaultSettings.quote);
      setPriceRangeEnabled(settings.price_range_enabled !== undefined ? settings.price_range_enabled : defaultSettings.price_range_enabled);
      setPriceRange({ 
        low: (settings.price_range && settings.price_range[0] !== undefined ? settings.price_range[0] : defaultSettings.price_range[0]).toString(), 
        high: (settings.price_range && settings.price_range[1] !== undefined ? settings.price_range[1] : defaultSettings.price_range[1]).toString() 
      });
    }
  }, [bot?.id]); // Only run when bot ID changes

  if (tileMode) {
    // Calculate profit/loss before rendering
    const profitLoss = bot.stats && {
      value: (parseFloat(bot.stats.currentValue) - parseFloat(bot.stats.totalInvested)).toFixed(2),
      percentage: ((parseFloat(bot.stats.currentValue) - parseFloat(bot.stats.totalInvested)) / parseFloat(bot.stats.totalInvested) * 100).toFixed(2),
      positive: parseFloat(bot.stats.currentValue) >= parseFloat(bot.stats.totalInvested)
    };

    return (
      <div 
        onClick={onClick} 
        className={`widget bot-tile ${botRunningClass}`}
      >
        <div className="bot-tile__header">
          <div className="bot-tile__ticker">
            <div className="bot-tile__ticker__currencies">{baseName}{quoteName}</div>
            <div className="bot-tile__ticker__exchange">DCA · {exchangeName}</div>
          </div>
          
          {bot.stats && bot.stats.currentValue && bot.stats.totalInvested && (
            <div className={`bot-tile__pnl ${profitLoss.positive ? 'text-success' : 'text-danger'}`}>
              <span className="widget__pnl__value">
                {profitLoss.positive ? '+' : ''}
                {profitLoss.percentage}%
              </span>
            </div>
          )}
        </div>
        
        <div className="bot-tile__controls">
            <div className="bot-tile__controls__feedback">
              {pending && nextResultFetchingTimestamp && 
                <FetchFromExchangeTimer bot={bot} callback={reload}/>
              }
              {working && nextTransactionTimestamp && 
                <Timer bot={bot} callback={reload}/>
              }
              {!working && isNotEmpty(errors) && <Errors data={errors}/>}
              <ProgressBar bot={bot} />
            </div>
        
            {isStarting && <StartingButton />}
            {(!isStarting && working) && 
              <div onClick={buttonClickHandler}>
                <StopButton onClick={() => handleStop(id)}/>
              </div>
            }
            {(!isStarting && pending) && <PendingButton />}
            {(!isStarting && !working && !pending) &&
              <div onClick={buttonClickHandler}>
                <StartButton 
                  settings={settings} 
                  getRestartType={getStartButtonType} 
                  onClickReset={_handleSubmit}
                  setShowInfo={setShowSmartIntervalsInfo} 
                  exchangeName={exchangeName} 
                  newSettings={newSettings()}
                />
              </div>
            }
        </div>
        
      </div>
    );
  }

  // Full view
  return (
    <div className="db-bots__item db-bot db-bot--dca">
      { apiKeyExists &&
        <div className="db-bot__header">

          {isStarting && <StartingButton/>}
          {(!isStarting && working) && <StopButton onClick={() => handleStop(id)}/>}
          {(!isStarting && pending) && <PendingButton/>}
          {(!isStarting && !working && !pending) &&
          <StartButton settings={settings} getRestartType={getStartButtonType} onClickReset={_handleSubmit}
                       setShowInfo={setShowSmartIntervalsInfo} exchangeName={exchangeName} newSettings={newSettings()}/>}
          <div className={`db-bot__infotext text-${colorClass}`}>

            <div className="db-bot__infotext__left bot-ticker">
              <span className="bot-ticker__exchange">{exchangeName}</span>
              <span className="bot-ticker__divider">:</span>
              <span className="bot-ticker__currencies">{baseName}{quoteName}</span>
            </div>
            {pending && nextResultFetchingTimestamp && <FetchFromExchangeTimer bot={bot} callback={reload}/>}
            {working && nextTransactionTimestamp && <Timer bot={bot} callback={reload}/>}
            {!working && isNotEmpty(errors) && <Errors data={errors}/>}
          </div>
        </div>
      }
      <div className="db-bot__progressbar-wrapper">
        <ProgressBar bot={bot} />
      </div>
      

      { !apiKeyExists &&
        <AddApiKey
          pickedExchangeName={exchangeName}
          handleReset={null}
          handleSubmit={addApiKeyHandler}
          handleRemove={() => removeInvalidApiKeys(exchangeId)}
          status={apiKeysState}
          botView={{baseName, quoteName}}
          type={'trading'}
        />
      }

      { apiKeyExists &&
        <div className="db-bot__form">
        <form>
          <div className="conversational flex-justify-center">
            <select
              value={type}
              onChange={handleTypeChange}
              className="sinput sinput--select"
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
            {isSellOffer()?
                <>
                  <input
                      type="text"
                      size={(price.length > 0) ? price.length : 3}
                      value={price}
                      className="sinput"
                      onChange={e => setPrice(e.target.value)}
                      disabled={working}
                  /> {baseName} {I18n.t('bots.for')}
                </>
                :
                <>
                  {baseName} {I18n.t('bots.for')} <input
                      type="text"
                      size={(price.length > 0) ? price.length : 3}
                      value={price}
                      className="sinput"
                      onChange={e => setPrice(e.target.value)}
                      disabled={working}
                  />
                </>

            }
            {quoteName} / <select
                value={interval}
                className="sinput sinput--select"
                onChange={e => setInterval(e.target.value)}
                disabled={working}
              >
              <option value="hour">{I18n.t('bots.hour')}</option>
              <option value="day">{I18n.t('bots.day')}</option>
              <option value="week">{I18n.t('bots.week')}</option>
              <option value="month">{I18n.t('bots.month')}</option>
            </select>
          </div>

          {showSubaccounts && <label
              className="alert alert-primary"
              disabled={!useSubaccount}
          >
            <input
                className="hide-when-running"
                type="checkbox"
                checked={useSubaccount}
                onChange={() => setUseSubaccounts(!useSubaccount)}
                disabled={working}
            />
            <div>
              <RawHTML tag="span">{splitTranslation(I18n.t('bots.subaccounts_info'))}</RawHTML>
              <select
                  value={selectedSubaccount}
                  onChange={e => setSelectedSubaccount(e.target.value)}
                  disabled={working}
                  className="bot-input bot-input--select bot-input--ticker bot-input--paper-bg"
              >
                {
                  subaccountsList.map( x => <option key={x} value={x}>{x}</option>)
                }
              </select>

            </div>
          </label>}

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
                  {getSmartIntervalsDisclaimer()}
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
              <RawHTML tag="span">{ I18n.t('bots.feecutter_html')}</RawHTML> <input
              type="text"
              value={percentage}
              size={(percentage.length > 0) ? percentage.length : 3}
              className="bot-input bot-input--sizable"
              onChange={e => setPercentage(e.target.value)}
              onBlur={validatePercentage}
              disabled={working || !showLimitOrders || !isLimitSelected()}
            /> % {isSellOffer() ? I18n.t('bots.above') : (isLegacySell() ? I18n.t('bots.above') :I18n.t('bots.below'))} {I18n.t('bots.price')}.

              { isLimitSelected() && <small className="hide-when-running"><LimitOrderNotice/></small> }
              { !showLimitOrders && <div className="bot input bot-input--pro-plan-only--before"><a href={`/${document.body.dataset.locale}/upgrade`}>Pro</a></div> }
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
              <RawHTML tag="span">{splitTranslation(I18n.t((isLegacySell() || isSellOffer()) ? 'bots.price_range_sell_html' :'bots.price_range_buy_html', {quote: quoteName, base: baseName}))[0]}</RawHTML>
              <input
                type="text"
                className="bot-input bot-input--sizable"
                value={priceRange.low}
                onChange={e => setPriceRange({low: e.target.value, high: priceRange.high})}
                disabled={working || !showLimitOrders}
                size={Math.max(priceRange.low.length, 1)}
              />
              <RawHTML tag="span">{splitTranslation(I18n.t((isLegacySell() || isSellOffer()) ? 'bots.price_range_sell_html' :'bots.price_range_buy_html', {quote: quoteName, base: baseName}))[1]}</RawHTML>
              <input
                type="text"
                className="bot-input bot-input--sizable"
                value={priceRange.high}
                onChange={e => setPriceRange({low: priceRange.low, high: e.target.value})}
                disabled={working || !showLimitOrders}
                size={ Math.max(priceRange.high.length, 1) }
              />
              <RawHTML tag="span">{splitTranslation(I18n.t((isLegacySell() || isSellOffer()) ? 'bots.price_range_sell_html' :'bots.price_range_buy_html', {quote: quoteName, base: baseName}))[2]}</RawHTML>
              { !showLimitOrders && <div className="bot input bot-input--pro-plan-only--before"><a href={`/${document.body.dataset.locale}/upgrade`} >Pro</a></div> }
            </div>
          </label>
        </form>
      </div>

      }
      <div className="bot-footer" hidden={working}>
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
  handleEdit: editTradingBot,
  fetchMinimums: getSmartIntervalsInfo,
  handleClick: openBot,
  clearBotErrors: clearErrors,
  fetchRestartParams: fetchRestartParams
})
export const TradingBot = connect(mapStateToProps, mapDispatchToProps)(BotTemplate)
