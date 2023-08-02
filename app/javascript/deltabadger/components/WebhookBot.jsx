import 'lodash';
import React, {useEffect, useState} from 'react';
import I18n from 'i18n-js';
import { connect } from 'react-redux';
import { startButtonType, StartButton, StartingButton, StopButton, RemoveButton, PendingButton } from './buttons';
import { Timer, FetchFromExchangeTimer } from './Timer';
import { ProgressBar } from './ProgressBar';
import CopyToClipboardText from './CopyToClipboardText';
import LimitOrderNotice from './BotForm/LimitOrderNotice';
import { isNotEmpty } from '../utils/array';
import {shouldRename, renameSymbol, renameCurrency, shouldShowSubaccounts} from '../utils/symbols';
import { RawHTML } from './RawHtml';
import { AddApiKey } from './BotForm/AddApiKey';
import { removeInvalidApiKeys, splitTranslation } from './helpers';

import {
  reloadBot,
  stopBot,
  removeBot,
  editWebhookBot,
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

  const isBuyOffer = () => settings.type === 'buy' || settings.type === 'buy_all';
  const isSellOffer = () => settings.type === 'sell' || settings.type === 'sell_all';
  const isBuySellType = (type) => type === 'buy' || type === 'sell';

  const [type, setType] = useState(settings.type);
  const [name, setName] = useState(settings.name);
  const [additionalType, setAdditionalType] = useState(settings.additional_type || (isBuyOffer() ? 'sell' : 'buy'));
  const [price, setPrice] = useState(settings.price);
  const [additionalPrice, setAdditionalPrice] = useState(settings.additional_price);
  const [minimumOrderParams, setMinimumOrderParams] = useState({});
  const [currencyOfMinimum, setCurrencyOfMinimum] = useState(settings.quote);
  const [triggerPossibility, setTriggerPossibility] = useState(settings.trigger_possibility);
  const [triggerUrl] = useState(settings.trigger_url);
  const [additionalTriggerUrl] = useState(settings.additional_trigger_url);
  const [additionalTypeEnabled, setAdditionalTypeEnabled] = useState(settings.additional_type_enabled || false);
  const [apiKeyExists, setApiKeyExists] = useState(true);
  const [apiKeysState, setApiKeysState] = useState(apiKeyStatus["ADD"]);

  const isStarting = startingBotIds.includes(id);
  const working = status === 'working'
  const pending = status === 'pending'

  const colorClass = settings.type === 'buy' ? 'success' : 'danger'
  const botOpenClass = open ? 'db-bot--active' : 'db-bot--collapsed'
  const botRunningClass = working ? 'bot--running' : 'bot--stopped'

  const disableSubmit = isBuySellType(type) && price.trim() === ''

  const isLimitSelected = () => type === 'limit'

  // const [showSubaccounts,setShowSubaccounts] = useState(false)

  const setLimitOrderCheckbox = () => {
    isLimitSelected() ? setType('market') : setType('limit')
  }

  const hasConfigurationChanged = () => {
    const newSettings= {
      order_type: type,
      // interval,
      price: price.trim(),
      // forceSmartIntervals,
      // smartIntervalsValue,
      percentage: isLimitSelected() ? percentage && percentage.trim() : undefined,
      // useSubaccount,
      // selectedSubaccount
    }

    const oldSettings = {
      order_type: settings.order_type,
      interval: settings.interval,
      price: settings.price.trim(),
      forceSmartIntervals: settings.force_smart_intervals,
      smartIntervalsValue: settings.smartIntervalsValue,
      percentage: settings.order_type === 'limit' ? percentage && percentage.trim() : undefined,
      useSubaccount: settings.useSubaccount,
      selectedSubaccount: settings.selectedSubaccount
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
    // if (disableSubmit) return

    const botParams = {
        id: bot.id,
        type,
        name,
        price: price.trim(),
        triggerPossibility,
        additionalTypeEnabled,
        additionalType,
        additionalPrice: additionalPrice?.trim()
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

  const baseName = shouldRename(exchangeName) ? renameSymbol(settings.base) : settings.base;
  const quoteName = shouldRename(exchangeName) ? renameSymbol(settings.quote) : settings.quote;

  const handleTypeChange = (e) => {
    setType(e.target.value)
    clearBotErrors(id)
  }

  // const newSettings = () => {
  //   const out = {
  //     price: price.trim(),
  //     forceSmartIntervals: forceSmartIntervals
  //   }
  //
  //   return out
  // }

  const getBotParams = () => {
    // debugger
    return {
        type,
        name,
        price: price.trim(),
        triggerPossibility,
        additionalTypeEnabled,
        additionalType,
        additionalPrice: additionalPrice?.trim()
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
  // const setSubaccounts = async () => {
  //   await API.getSubaccounts(exchangeId).then(data => {
  //     setSubaccountsList(data.data['subaccounts']);
  //     setShowSubaccounts(data.data['subaccounts'].length > 0 && shouldShowSubaccounts(exchangeName));
  //     setSelectedSubaccount(settings.use_subaccount ? settings.selected_subaccount : (data.data['subaccounts'].length > 0 ? data.data['subaccounts'][0] : ''));
  //   })
  // }

  // useEffect(() => {
  //   async function fetchSmartIntervalsInfo()  {
  //     const data = await fetchMinimums(getBotParams())
  //     // if (isLimitOrderDefinedInBase(exchangeName) && isLimitSelected()) {
  //     //   data.data.minimum = data.data.minimum_limit
  //     //   data.data.side = 'base'
  //     // }
  //
  //     const minimum = data.data.minimum
  //     const currency = data.data.side === 'base' ? renameCurrency(settings.base, exchangeName) : renameCurrency(settings.quote, exchangeName)
  //
  //     // await setSubaccounts()
  //     setMinimumOrderParams(getMinimumOrderParams(data))
  //     // if (smartIntervalsValue === "0") {
  //     //   setSmartIntervalsValue(minimum.toString())
  //     // }
  //     setCurrencyOfMinimum(currency)
  //   }
  //
  //   fetchSmartIntervalsInfo()
  // }, [type]);

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
    const exchange = exchanges.find(e => exchangeId === e.id) || {trading_key_status: true, withdrawal_key_status: true, webhook_key_status: true}
    const keyStatus = exchange.trading_key_status

    // debugger

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

  const webhookUrl = `${window.location.origin}/h/${triggerUrl}`;
  const additionalWebhookUrl = `${window.location.origin}/h/${settings.additional_trigger_url}`;

  return (
    <div onClick={() => handleClick(id)} className={`db-bots__item db-bot db-bot--webhook db-bot--setup-finished ${botOpenClass} ${botRunningClass}`}>
      { apiKeyExists &&
        <div className="db-bot__header">
          {isStarting && <StartingButton/>}
          {(!isStarting && working) && <StopButton onClick={() => handleStop(id)}/>}
          {(!isStarting && pending) && <PendingButton/>}
          {(!isStarting && !working && !pending) &&
          <StartButton settings={settings} getRestartType={getStartButtonType} onClickReset={_handleSubmit}
                       setShowInfo={setShowSmartIntervalsInfo} exchangeName={exchangeName} />}
          <div className={`db-bot__infotext text-${colorClass}`}>
            <div className="db-bot__infotext__left">
              <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                <path stroke="#2948A1" strokeLinecap="round" strokeWidth="2" d="M7.8 10.8a6 6 0 0 1 8.4 0M5 8c3.8-4 10.2-4 14 0"/>
                <circle cx="12" cy="15" r="2" stroke="#2948A1" strokeWidth="2"/>
              </svg>
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
          type={'webhook'}
        />
      }

      { apiKeyExists &&
        <div className="db-bot__form">
        <form className="db-bot__form__schedule">
          <div className="form-inline mb-4 pb-5">
            {working ? <b className="form-group mr-2">{name}</b> : <div className="form-group mr-2">{I18n.t('bots.name')}</div>}
            {working ? null :
              <div className="form-group">
                <input
                    type="text"
                    min="5"
                    value={name}
                    size={(name.length > 0) ? name.length : 1}
                    onChange={e => setName(e.target.value)}
                    className="bot-input bot-input--sizable bot-input--paper-bg"
                    disabled={working}
                />
            </div>
            }
          </div>

          <div className="form-inline mb-5">
            <div className="form-group mr-2">
              <select
                value={type}
                onChange={handleTypeChange}
                className="bot-input bot-input--select bot-input--order-type bot-input--paper-bg"
                disabled={working}
              >
                {isSellOffer() ? <>
                    <option value="sell">{I18n.t('bots.sell')}</option>
                    <option value="sell_all">{I18n.t('bots.sell_all')}</option>
                  </> : <>
                    <option value="buy">{I18n.t('bots.buy')}</option>
                    <option value="buy_all">{I18n.t('bots.buy_all')}</option>
                  </>
                }
              </select>
            </div>
            {isSellOffer()?
              <>
                {isBuySellType(type) && <div className="form-group mr-2">
                  <input
                      type="text"
                      size={(price.length > 0) ? price.length : 1}
                      value={price}
                      className="bot-input bot-input--sizable bot-input--paper-bg"
                      onChange={e => setPrice(e.target.value)}
                      disabled={working}
                  />
                </div>}
                <div className="form-group mr-2"> {baseName} {I18n.t('bots.for')}</div>
              </> : <>
                <div className="form-group mr-2"> {baseName} {I18n.t('bots.for')}</div>
                {isBuySellType(type) && <div className="form-group mr-2">
                  <input
                      type="text"
                      size={(price.length > 0) ? price.length : 1}
                      value={price}
                      className="bot-input bot-input--sizable bot-input--paper-bg"
                      onChange={e => setPrice(e.target.value)}
                      disabled={working}
                  />
                </div>}
              </>
            }
            <div className="form-group mr-2"> {quoteName}</div>

            <div className="form-group mr-2">
              <select
                  value={triggerPossibility}
                  onChange={e => setTriggerPossibility(e.target.value)}
                  className="bot-input bot-input--select bot-input--interval bot-input--paper-bg"
                  disabled={working}
              >
                <option value="first_time">{I18n.t('bots.first_time')}</option>
                <option value="every_time">{I18n.t('bots.every_time')}</option>
              </select>
            </div>

            {triggerUrl && <>
              <div className="form-group mr-2">
                {I18n.t('bots.triggered_title')}
              </div>
              <div className="form-group bot-input bot-input--sizable bot-input--paper-bg">
                <CopyToClipboardText text={webhookUrl} feedbackText={I18n.t('bots.webhook_has_been_copied')} />
              </div>
            </>}
          </div>

          {working ? null : (

            <div className="form-inline mb-4">
              <label className="form-group mr-2" for="twoWaysBot">{I18n.t('bots.additional_title')}</label>
              <div className="form-group mr-2">
                <input
                    type="checkbox"
                    id="twoWaysBot"
                    checked={additionalTypeEnabled}
                    onChange={() => setAdditionalTypeEnabled(!additionalTypeEnabled)}
                    disabled={working}
                />
              </div>
            </div>

          )}

          {!additionalTypeEnabled ? null : (

            <div className="form-inline">
              <div className="form-group mr-2">
                <select
                    value={additionalType}
                    onChange={e => setAdditionalType(e.target.value)}
                    className="bot-input bot-input--select bot-input--order-type bot-input--paper-bg"
                    disabled={!additionalTypeEnabled || working}
                >
                  {isBuyOffer()?
                      <>
                        <option value="sell">{I18n.t('bots.sell')}</option>
                        <option value="sell_all">{I18n.t('bots.sell_all')}</option>
                      </> : <>
                        <option value="buy">{I18n.t('bots.buy')}</option>
                        <option value="buy_all">{I18n.t('bots.buy_all')}</option>
                      </>
                  }
                </select>
              </div>
              {isSellOffer()?
                  <>
                    <div className="form-group mr-2">{baseName}</div>
                    <div className="form-group mr-2">{I18n.t('bots.for')}</div>
                    {isBuySellType(additionalType) && <div className="form-group mr-2">
                      <input
                          type="text"
                          min="1"
                          size={(additionalPrice && additionalPrice.length > 0) ? additionalPrice.length : 3 }
                          value={additionalPrice}
                          onChange={e => setAdditionalPrice(e.target.value)}
                          className="bot-input bot-input--sizable bot-input--paper-bg"
                          disabled={working}
                      />
                    </div>}
                    <div className="form-group mr-2">{quoteName}</div>
                    <div className="form-group mr-2">{I18n.t('bots.'+triggerPossibility)}</div>
                  </> : <>
                    {isBuySellType(additionalType) && <div className="form-group mr-2">
                      <input
                          type="text"
                          min="1"
                          size={(additionalPrice && additionalPrice.length > 0) ? additionalPrice.length : 3 }
                          value={additionalPrice}
                          onChange={e => setAdditionalPrice(e.target.value)}
                          className="bot-input bot-input--sizable bot-input--paper-bg"
                          disabled={working}
                      />
                    </div>}
                    <div className="form-group mr-2">{baseName}</div>
                    <div className="form-group mr-2">{I18n.t('bots.for')}</div>
                    <div className="form-group mr-2">{quoteName}</div>
                    <div className="form-group mr-2">{I18n.t('bots.'+triggerPossibility)}</div>
                  </>
              }
              {settings.additional_trigger_url && <>
                <div className="form-group mr-2">
                  {I18n.t('bots.triggered_title')}
                </div>
                <div className="form-group bot-input bot-input--sizable bot-input--paper-bg">
                  <CopyToClipboardText text={additionalWebhookUrl} feedbackText={I18n.t('bots.webhook_has_been_copied')} />
                </div>
              </>}
            </div>

          )}
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
  handleEdit: editWebhookBot,
  fetchMinimums: getSmartIntervalsInfo,
  handleClick: openBot,
  clearBotErrors: clearErrors,
  fetchRestartParams: fetchRestartParams
})
export const WebhookBot = connect(mapStateToProps, mapDispatchToProps)(BotTemplate)
