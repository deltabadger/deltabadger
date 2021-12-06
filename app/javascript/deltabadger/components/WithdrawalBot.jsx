import 'lodash'
import React, {useEffect, useState} from 'react';
import I18n from 'i18n-js'
import { connect } from 'react-redux';
import { StartingButton, StopButton, RemoveButton, PendingButton } from './buttons'
import { Timer } from './Timer';
import { ProgressBar } from './ProgressBar';
import { isNotEmpty } from '../utils/array';
import {shouldRename, renameSymbol} from "../utils/symbols";
import { RawHTML } from './RawHtml'
import { AddApiKey } from "./BotForm/AddApiKey";
import { removeInvalidApiKeys, splitTranslation } from "./helpers";

import {
  reloadBot,
  stopBot,
  removeBot,
  openBot,
  clearErrors,
  fetchRestartParams,
  getSmartIntervalsInfo,
  getWithdrawalMinimums,
  editWithdrawalBot
} from '../bot_actions'
import API from "../lib/API";
import {PercentageProgress} from "./PercentageProgress";

const apiKeyStatus = {
  ADD: 'add_api_key',
  VALIDATING: 'validating_api_key',
  INVALID: 'invalid_api_key'
}

const BotTemplate = ({
  bot,
  errors = [],
  startingBotIds,
  handleStop,
  handleRemove,
  handleClick,
  handleEdit,
  reload,
  reloadPage,
  open,
  fetchExchanges,
  exchanges,
  apiKeyTimeout,
  getMinimums
}) => {
  const { id, settings, status, exchangeName, exchangeId, nextTransactionTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}

  const [threshold, setThreshold] = useState(settings.threshold);
  const [thresholdEnabled, setThresholdEnabled] = useState(settings.threshold_enabled);
  const [interval, setInterval] = useState(settings.interval);
  const [intervalEnabled, setIntervalEnabled] = useState(settings.interval_enabled);
  const [minimum, setMinimum] = useState("0")
  const [apiKeyExists, setApiKeyExists] = useState(true);
  const [apiKeysState, setApiKeysState] = useState(apiKeyStatus["ADD"]);

  const isStarting = startingBotIds.includes(id);
  const working = status === 'working'
  const pending = status === 'pending'

  const colorClass = 'success'
  const botOpenClass = open ? 'db-bot--active' : 'db-bot--collapsed'
  const botRunningClass = working ? 'bot--running' : 'bot--stopped'

  const disableSubmit = threshold.trim() === ''

  const currencyName = shouldRename(exchangeName) ? renameSymbol(settings.currency) : settings.currency

  const _handleSubmit = () => {
    if (disableSubmit) return

    const botParams = {
      id: bot.id,
      threshold,
      thresholdEnabled,
      interval,
      intervalEnabled
    }

    setTimeout(() => reload(bot), 3000)
    handleEdit(botParams)
  }

  // Shows the first (major) error
  const Errors = ({ data }) => (
    <div className="db-bot__infotext__right">
      { data[0] }
    </div>
  )

  const keyOwned = (status) => status === 'correct'
  const keyPending = (status) => status === 'pending'
  const keyInvalid = (status) => status === 'incorrect'

  const keyExists = () => {
    // we have to assume that the key exists to fix unnecessary form rendering
    const exchange = exchanges.find(e => exchangeId === e.id) || {trading_key_status: true, withdrawal_key_status: true}
    const keyStatus = exchange.withdrawal_key_status
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

  const addApiKeyHandler = (key, secret, passphrase, germanAgreement, keyType) => {
    setApiKeysState(apiKeyStatus["VALIDATING"])
    API.createApiKey({ key, secret, passphrase, germanAgreement, exchangeId: exchangeId, type: keyType}).then(response => {
      fetchExchanges()
    }).catch(() => {
      setApiKeysState(apiKeyStatus["INVALID"])
    })
  }
  useEffect( () => {
    async function fetchMinimums () {
      const minimums = await getMinimums(exchangeId, settings.currency)
      setMinimum(minimums.minimum.toString())
    }

    fetchMinimums()
  }, [apiKeyExists,])


  return (
    <div onClick={() => handleClick(id)} className={`db-bots__item db-bot db-bot--dca db-bot--setup-finished ${botOpenClass} ${botRunningClass}`}>
      { apiKeyExists &&
        <div className="db-bot__header">
          { isStarting && <StartingButton/> }
          {(!isStarting && working) && <StopButton onClick={() => handleStop(id)}/>}
          {(!isStarting && pending) && <PendingButton/>}
          {(!isStarting && !working && !pending) &&
            <div onClick={_handleSubmit} className={`btn ${disableSubmit ? 'btn-outline-secondary disabled' : 'btn-outline-success'}`}>
              <span className="d-none d-sm-inline">{I18n.t('bots.start')}</span>
              <svg className="btn__svg-icon db-svg-icon db-svg-icon--play" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M8 6.8v10.4a1 1 0 001.5.8l8.2-5.2a1 1 0 000-1.7L9.5 6a1 1 0 00-1.5.8z"/></svg>
            </div>
          }
          <div className={`db-bot__infotext text-${colorClass}`}>
            <div className="db-bot__infotext__left">
              {exchangeName}:{currencyName}
            </div>
            {working && !intervalEnabled && <PercentageProgress bot={bot} callback={reload}/>}
            {working && intervalEnabled && nextTransactionTimestamp && <Timer bot={bot} callback={reload}/>}
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
          botView={{currencyName}}
          type={'withdrawal'}
        />
      }

      { apiKeyExists &&
        <div className="db-bot__form">
          <form>
            <div className="form-inline db-bot__form__schedule">
              <div className="form-group mr-2">{I18n.t('bots.setup.withdrawal_html',
                {currency: currencyName, address: settings.address}).replaceAll(/<\/?split>/g, '')}</div>
            </div>

            <label
              className="alert alert-primary"
              disabled={!thresholdEnabled}
            >
              <input
                type="checkbox"
                checked={thresholdEnabled}
                onChange={() => setThresholdEnabled(!thresholdEnabled)}
                disabled={working}
              />
              <div>
                <RawHTML tag="span">{splitTranslation(I18n.t('bots.withdraw_threshold_html', {currency: currencyName}))[0]}</RawHTML>
                <input
                  type="text"
                  size={(threshold.length > 0) ? threshold.length : 3 }
                  className="bot-input bot-input--sizable"
                  value={threshold}
                  onChange={e => setThreshold(e.target.value)}
                  disabled={working}
                />
                <RawHTML tag="span">{splitTranslation(I18n.t('bots.withdraw_threshold_html', {currency: currencyName}))[1]}</RawHTML>

                <small className="hide-when-running hide-when-disabled">
                  <div>
                    <sup>*</sup>{I18n.t('bots.minimum_withdrawal_disclaimer', {currency: currencyName, minimum: minimum})}
                  </div>
                </small>
              </div>
            </label>

            <label
              className="alert alert-primary"
              disabled={!intervalEnabled}
            >
              <input
                type="checkbox"
                checked={intervalEnabled}
                onChange={() => setIntervalEnabled(!intervalEnabled)}
                disabled={working}
              />
              <div>
                <RawHTML tag="span">{splitTranslation(I18n.t('bots.withdraw_interval_html'))[0]}</RawHTML>
                <input
                  type="text"
                  size={(interval.length > 0) ? interval.length : 3 }
                  className="bot-input bot-input--sizable"
                  value={interval}
                  onChange={e => setInterval(e.target.value)}
                  disabled={working}
                />
                <RawHTML tag="span">{splitTranslation(I18n.t('bots.withdraw_interval_html'))[1]}</RawHTML>
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
  handleEdit: editWithdrawalBot,
  fetchMinimums: getSmartIntervalsInfo,
  handleClick: openBot,
  clearBotErrors: clearErrors,
  fetchRestartParams: fetchRestartParams,
  getMinimums: getWithdrawalMinimums
})
export const WithdrawalBot = connect(mapStateToProps, mapDispatchToProps)(BotTemplate)
