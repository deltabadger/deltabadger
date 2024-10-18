import React, {useEffect, useRef, useState} from 'react'
import {Breadcrumbs} from './Breadcrumbs'
import {Progressbar} from './Progressbar'
import {getSpecialSymbols, renameCurrency, renameSymbol, shouldRename} from "../../utils/symbols";
import I18n from "i18n-js";
import {RawHTML} from "../RawHtml";
import {WithdrawalAddressInstructions} from "./WithdrawalAddressInstructions";

export const ConfigureWithdrawalBot = ({ currentExchange, handleReset, handleSubmit, getMinimums, disable, errors }) => {
  const shouldRenameSymbols = shouldRename(currentExchange.name)

  const compareSymbols = (x, y) => {
    if (shouldRenameSymbols) {
      return renameSymbol(x).localeCompare(renameSymbol(y))
    } else {
      return x.localeCompare(y)
    }
  }

  const sortSymbols = (symbols, specialSymbols) => {
    const specialSymbolsOrEmpty = specialSymbols.filter(s => symbols.includes(s))
    const otherSymbols = symbols.filter(s => !(specialSymbols.includes(s)))
    return [...specialSymbolsOrEmpty, ...otherSymbols.sort(compareSymbols)]
  }

  const uniqueArray = (array) => [...new Set(array)]
  const CURRENCIES = sortSymbols(uniqueArray(currentExchange.withdrawal_currencies), getSpecialSymbols(currentExchange.name, true))
  const WALLET_ADDRESSES = uniqueArray(currentExchange.withdrawal_addresses)

  const [addressesForCurrency, setAddressesForCurrency] = useState([])
  const [address, setAddress] = useState('');
  const [currency, setCurrency] = useState(CURRENCIES[0]);
  const [threshold, setThreshold] = useState("0");
  const [thresholdEnabled, setThresholdEnabled] = useState(true);
  const [interval, setInterval] = useState("0");
  const [intervalEnabled, setIntervalEnabled] = useState(false);
  const [minimum, setMinimum] = useState("0")
  const node = useRef()

  const currencyName = shouldRename(currentExchange.name) ? renameSymbol(currency) : currency

  const ResetButton = () => (
    <div
      onClick={() => handleReset()}
      className="sbutton sbutton--link"
    >
      <i className="material-icons">close</i>
      <span>Cancel</span>
    </div>
  )

  const handleClickOutside = e => {
    if (node.current && node.current.contains(e.target)) {
      return;
    }
  };

  const exchangeWithoutAddressEndpoint = () => {
    return currentExchange.name.toLowerCase() === 'kraken'
  }

  const existsAddress = () => {
    return !exchangeWithoutAddressEndpoint() && address !== ''
  };

  const filterAddressesForCurrency = () => {
    let addresses = WALLET_ADDRESSES.filter(a => a.currency === currency).map(a => a.address)
    setAddressesForCurrency(addresses)
    setAddress(addresses[0] || '')
  };

  useEffect( () => {
    filterAddressesForCurrency()
    async function fetchMinimums () {
      const minimums = await getMinimums(currentExchange.id, currency)
      setMinimum(minimums.minimum.toString())
      if (currentExchange.name === 'Kraken'){
        setThreshold(minimums.minimum.toString())
      }
    }

    fetchMinimums()
  }, [currency,])

  useEffect(() => {
    document.addEventListener("mousedown", handleClickOutside);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
    };
  }, []);

  const disableSubmit = disable || threshold.trim() === '' || address.trim() === ''

  const getBotParams = () => {
    const botParams = {
      currency,
      address,
      threshold: threshold.trim(),
      thresholdEnabled,
      interval: interval.trim(),
      intervalEnabled,
      botType: 'withdrawal',
    }

    return botParams
  }

  const _handleSubmit = (evt) => {
    evt.preventDefault();
    const botParams = getBotParams()
    !disableSubmit && handleSubmit(botParams);
  }

  const splitTranslation = (s) => {
    return s.split(/<split>.*?<\/split>/)
  }

  const getMinimumDisclaimer = () => {
    return currentExchange.name === 'kraken' ?
      I18n.t('bots.minimum_withdrawal_disclaimer', {currency: currencyName, minimum: minimum}) :
      I18n.t('bots.minimum_withdrawal_disclaimer_usd', {minimum: minimum, exchange: currentExchange.name})
  }

  return (
    <div className="db-bots__item db-bot db-bot--withdrawal db-bot--setup db-bot--ready db-bot--active">

      <div className="db-bot__header">
        <Breadcrumbs step={3} />
        <div onClick={_handleSubmit} className={`sbutton ${disableSubmit ? 'sbutton--outline sbutton--disabled' : 'sbutton--success'}`}>
          <div className="animicon animicon--start">
            <div className="animicon__a"></div>
            <div className="animicon__b"></div>
          </div>
        </div>
    </div>

      <Progressbar value={66} />

      <div className="db-bot__form">
        <div className="db-bot__alert text-danger">{ errors }</div>
        <form>
          <div className="form-inline db-bot__form__schedule">
            <div className="form-group mr-2">{splitTranslation(I18n.t('bots.setup.withdrawal_html', {currency: currencyName, address: address}))[0]}</div>
            <div className="form-group mr-2">
              <select
                value={currency}
                onChange={e => setCurrency(e.target.value)}
                className="bot-input bot-input--select bot-input--ticker bot-input--paper-bg"
              >
                {
                  CURRENCIES.map(c =>
                    (<option key={c} value={c}>{renameSymbol(c)}</option>)
                  )
                }
              </select>
            </div>
            { existsAddress() &&
              <>
                <div className="form-group mr-2">{splitTranslation(I18n.t('bots.setup.withdrawal_html', {currency: currencyName, address: address}))[1]}</div>
                <div className="form-group mr-2">
                  <select
                    value={address}
                    onChange={e => setAddress(e.target.value)}
                    className="bot-input bot-input--select bot-input--ticker bot-input--paper-bg"
                  >
                    {
                      addressesForCurrency.map(c =>
                        (<option key={c} value={c}>{renameSymbol(c)}</option>)
                      )
                    }
                  </select>
                </div>
                <div className="form-group mr-2">{splitTranslation(I18n.t('bots.setup.withdrawal_html', {currency: currencyName, address: address}))[2]}</div>
              </>
            }
            { (!existsAddress() && !exchangeWithoutAddressEndpoint()) &&
              <div className="form-group mr-2">{I18n.t('bots.setup.no_wallet_found', {exchangeName: currentExchange.name})}</div>
            }
            { currentExchange.name.toLowerCase() === 'kraken' &&
              <>
                <div className="form-group mr-2">{splitTranslation(I18n.t('bots.setup.withdrawal_html', {currency: currencyName, address: address}))[1]}</div>
                <div className="form-group mr-2">
                  <input
                    type="text"
                    size={(address.length > 0) ? address.length : 3 }
                    value={address}
                    onChange={e => setAddress(e.target.value)}
                    className="bot-input bot-input--sizable bot-input--paper-bg"
                  />
                </div>
                <div className="form-group mr-2">{splitTranslation(I18n.t('bots.setup.withdrawal_html', {currency: currencyName, address: address}))[2]}</div>
              </>
            }
          </div>

          <label
            className="alert alert-primary"
            disabled={!thresholdEnabled}
          >
            <input
              type="checkbox"
              checked={thresholdEnabled}
              onChange={() => setThresholdEnabled(!thresholdEnabled)}
            />
            <div>
              <RawHTML tag="span">{splitTranslation(I18n.t('bots.withdraw_threshold_html', {currency: currencyName}))[0]}</RawHTML>
              <input
                type="text"
                size={(threshold.length > 0) ? threshold.length : 3 }
                className="bot-input bot-input--sizable"
                value={threshold}
                onChange={e => setThreshold(e.target.value)}
              />
              <RawHTML tag="span">{splitTranslation(I18n.t('bots.withdraw_threshold_html', {currency: currencyName}))[1]}</RawHTML>

              <small className="hide-when-running hide-when-disabled">
                <div>
                  {getMinimumDisclaimer()}
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
            />
            <div>
              <RawHTML tag="span">{splitTranslation(I18n.t('bots.withdraw_interval_html'))[0]}</RawHTML>
              <input
                type="text"
                size={(interval.length > 0) ? interval.length : 3 }
                className="bot-input bot-input--sizable"
                value={interval}
                onChange={e => setInterval(e.target.value)}
              />
              <RawHTML tag="span">{splitTranslation(I18n.t('bots.withdraw_interval_html'))[1]}</RawHTML>
            </div>
          </label>
        </form>
      </div>

      <WithdrawalAddressInstructions exchangeName={currentExchange.name} type={'withdrawal_address'}/>

      <div className="bot-footer">
        <ResetButton />
      </div>
    </div>
  )
}
