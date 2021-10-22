import React, {useEffect, useRef, useState} from 'react'
import {Breadcrumbs} from './Breadcrumbs'
import {Progressbar} from './Progressbar'
import LimitOrderNotice from "./LimitOrderNotice";
import {getSpecialSymbols, renameCurrency, renameSymbol, shouldRename} from "../../utils/symbols";
import I18n from "i18n-js";
import {RawHTML} from "../RawHtml";
import API from "../../lib/API";
import {StartButton} from "../buttons";

export const ConfigureWithdrawalBot = ({ currentExchange, handleReset, handleSubmit, disable, errors }) => {
  const shouldRenameSymbols = shouldRename(currentExchange.name)

  const compareSymbols = (x, y) => {
    if (shouldRenameSymbols) {
      return renameSymbol(x).localeCompare(renameSymbol(y))
    } else {
      return x.localeCompare(y)
    }
  }

  const uniqueArray = (array) => [...new Set(array)]
  const CURRENCIES = uniqueArray(currentExchange.withdrawal_currencies)
  const WALLET_ADDRESSES = uniqueArray(currentExchange.withdrawal_addresses)

  const [addressesForCurrency, setAddressesForCurrency] = useState([])
  const [address, setAddress] = useState('');
  const [currency, setCurrency] = useState(CURRENCIES[0]);
  const [threshold, setThreshold] = useState("0");
  const [thresholdEnabled, setThresholdEnabled] = useState(true);
  const [interval, setInterval] = useState("0");
  const [intervalEnabled, setIntervalEnabled] = useState(false);
  const node = useRef()

  const ResetButton = () => (
    <div
      onClick={() => handleReset()}
      className="btn btn-link btn--reset btn--reset-back"
    >
      <i className="material-icons-round">close</i>
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

  const _handleSubmit = (evt, smartIntervalsValue) => {
    evt.preventDefault();
    const botParams = getBotParams()
    !disableSubmit && handleSubmit(botParams);
  }

  return (
    <div className="db-bots__item db-bot db-bot--dca db-bot--setup db-bot--ready db-bot--active">

      <div className="db-bot__header">
        <Breadcrumbs step={2} />
        <div onClick={_handleSubmit} className={`btn ${disableSubmit ? 'btn-outline-secondary disabled' : 'btn-outline-success'}`}>
          <span className="d-none d-sm-inline">Start</span>
          <svg className="btn__svg-icon db-svg-icon db-svg-icon--play" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M8 6.8v10.4a1 1 0 001.5.8l8.2-5.2a1 1 0 000-1.7L9.5 6a1 1 0 00-1.5.8z"/></svg>
        </div>
        <div className="db-bot__infotext"/>
      </div>

      <Progressbar value={66} />

      <div className="db-bot__form">
        <div className="db-bot__alert text-danger">{ errors }</div>
        <form>
          <div className="form-inline db-bot__form__schedule">
            <div className="form-group mr-3">Withdraw </div>
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
                <div className="form-group mr-3"> to </div>
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
                <div className="form-group mr-3"> wallet.</div>
              </>
            }
            { (!existsAddress() && !exchangeWithoutAddressEndpoint()) &&
              <div className="form-group mr-3"> No wallet found. Go to {currentExchange.name} and add a withdrawal wallet.</div>
            }
            { currentExchange.name.toLowerCase() === 'kraken' &&
              <>
                <div className="form-group mr-3"> to </div>
                <div className="form-group mr-3">
                  <input
                    type="text"
                    size={(address.length > 0) ? address.length : 3 }
                    value={address}
                    onChange={e => setAddress(e.target.value)}
                    className="bot-input bot-input--sizable bot-input--paper-bg"
                  />
                </div>
                <div className="form-group mr-3"> wallet.</div>
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
            <RawHTML tag="span">Withdraw when at least </RawHTML>
              <input
                type="text"
                size={(threshold.length > 0) ? threshold.length : 3 }
                className="bot-input bot-input--sizable"
                value={threshold}
                onChange={e => setThreshold(e.target.value)}
              />
              <RawHTML tag="span">{` ${currency} is available`}</RawHTML>
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
              <RawHTML tag="span">Withdraw every </RawHTML>
              <input
                type="text"
                size={(interval.length > 0) ? interval.length : 3 }
                className="bot-input bot-input--sizable"
                value={interval}
                onChange={e => setInterval(e.target.value)}
              />
              <RawHTML tag="span"> days.</RawHTML>
            </div>
          </label>
        </form>
      </div>

      <div className="db-bot__footer">
        <ResetButton />
      </div>
    </div>
  )
}
