import React, {useState, useEffect, useRef} from 'react'
import { Breadcrumbs } from './Breadcrumbs'
import { Progressbar } from './Progressbar'
import LimitOrderNotice from "./LimitOrderNotice";
import {shouldRename, renameSymbol, getSpecialSymbols, renameCurrency} from "../../utils/symbols";
import I18n from "i18n-js";
import {RawHTML} from "../RawHtml";

export const ConfigureBot = ({ showLimitOrders, currentExchange, handleReset, handleSubmit, handleSmartIntervalsInfo, setShowInfo, disable, errors }) => {
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
  const BASES = sortSymbols(uniqueArray(currentExchange.symbols.map(s => s.base)), getSpecialSymbols(currentExchange.name, true))
  const QUOTES = sortSymbols(uniqueArray(currentExchange.symbols.map(s => s.quote)), getSpecialSymbols(currentExchange.name, false))

  const ALL_BASES = sortSymbols(uniqueArray(currentExchange.all_symbols.map(s => s.base)), getSpecialSymbols(currentExchange.name, true))
  const OTHER_BASES = ALL_BASES.filter(s => !(BASES.includes(s)))

  const [isOpen, setOpen] = useState(false);
  const [type, setType] = useState("market_buy");
  const [price, setPrice] = useState("");
  const [base, setBase] = useState(BASES[0]);
  const [quote, setQuote] = useState(QUOTES[0]);
  const [minimumOrderParams, setMinimumOrderParams] = useState({});
  const [interval, setInterval] = useState("hour");
  const [percentage, setPercentage] = useState("0");
  const [dontShowInfo, setDontShowInfo] = useState(false)
  const [forceSmartIntervals, setForceSmartIntervals] = useState(false);
  const node = useRef()

  const validQuotesForSelectedBase = () => {
    const symbols = currentExchange.symbols
    return QUOTES.filter(quote => symbols.find(symbol => symbol.base === base && symbol.quote === quote ))
  }

  const setFirstValidQuoteIfUnavailable = () => {
    const validQuotes = validQuotesForSelectedBase()
    if (validQuotes.includes(quote)) return;

    setQuote(validQuotes[0])
  }

  useEffect(() => {
    setFirstValidQuoteIfUnavailable()
  }, [base]);

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
    setOpen(false)
  };

  useEffect(() => {
    document.addEventListener("mousedown", handleClickOutside);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
    };
  }, []);

  const disableSubmit = disable || price.trim() === ''

  const _handleSmartIntervalsInfo = (evt) => {
    evt.preventDefault();
    const botParams = {
      type,
      base,
      quote,
      interval,
      forceSmartIntervals,
      price: price.trim(),
      percentage: isLimitOrder() ? percentage.trim() : undefined,
      botType: 'free',
    }

    return handleSmartIntervalsInfo(botParams).then((data) => {
      if (data.data.showSmartIntervalsInfo) {
        const minimumOrderParams = {
          value: data.data.minimum >= 1 ? Math.floor(data.data.minimum) : data.data.minimum,
          currency: data.data.side === 'base' ? renameCurrency(base, currentExchange.name) : renameCurrency(quote, currentExchange.name),
          showQuote: data.data.side === 'base',
          quoteValue: data.data.minimumQuote
        }
        setMinimumOrderParams(minimumOrderParams)
        setOpen(true);
      } else {
        _handleSubmit(evt)
      }
    })
  }

  const _setShowSmartIntervalsInfo = () => {
    setShowInfo()
  }

  const _handleInfoSubmit = (evt) => {
    if(!isOpen){
      return;
    }

    setOpen(false)

    if (dontShowInfo) {
      _setShowSmartIntervalsInfo()
    }

    _handleSubmit(evt)
  }

  const _handleSubmit = (evt) => {
    setOpen(false)
    evt.preventDefault();
    const botParams = {
      type,
      base,
      quote,
      interval,
      forceSmartIntervals,
      price: price.trim(),
      percentage: isLimitOrder() ? percentage.trim() : undefined,
      botType: 'free',
    }
    !disableSubmit && handleSubmit(botParams);
  }

  const isLimitOrder = () => type === 'limit_buy' || type === 'limit_sell'

  const isSellOffer = () => type === 'market_sell' || type === 'limit_sell'

  const getApproximateValue = (params) => {
    return params.showQuote ? ` (~${minimumOrderParams.quoteValue}${renameCurrency(quote, currentExchange.name)})` : ""
  }

  const getSmartIntervalsInfo = () => {
    if (forceSmartIntervals) {
      return I18n.t('bots.setup.smart_intervals.info_html.force_smart_intervals', {base: renameCurrency(base, currentExchange.name), quote: renameCurrency(quote, currentExchange.name), exchangeName: currentExchange.name, minimumValue: minimumOrderParams.value, minimumCurrency: minimumOrderParams.currency, approximatedQuote: getApproximateValue(minimumOrderParams)})
    }

    return I18n.t('bots.setup.smart_intervals.info_html.other', {base: renameCurrency(base, currentExchange.name), quote: renameCurrency(quote, currentExchange.name), exchangeName: currentExchange.name, minimumValue: minimumOrderParams.value, minimumCurrency: minimumOrderParams.currency, approximatedQuote: getApproximateValue(minimumOrderParams)})
  }

  return (
    <div className="db-bots__item db-bot db-bot--dca db-bot--setup db-bot--ready db-bot--active">
      { isOpen &&
        <div ref={node} className="db-bot__modal">
          <div className="db-bot__modal__content">
            <RawHTML tag="p">{getSmartIntervalsInfo()}</RawHTML>
            <label className="form-inline mx-4 mt-4 mb-2">
              <input
                type="checkbox"
                checked={dontShowInfo}
                onChange={() => setDontShowInfo(!dontShowInfo)}
                className="mr-2" />
              <span>{I18n.t('bots.setup.smart_intervals.dont_show_again')}</span>
            </label>

            <div className="db-bot__modal__btn-group">
              <div onClick={() => {
                setOpen(false)
              }} className="btn btn-outline-primary">Cancel
              </div>
              <div onClick={_handleInfoSubmit} className="btn btn-success">
                I understand
              </div>
            </div>
          </div>
        </div>
      }

      <div className="db-bot__header">
        <Breadcrumbs step={2} />
        <div onClick={_handleSmartIntervalsInfo} className={`btn ${disableSubmit ? 'btn-outline-secondary disabled' : 'btn-outline-success'}`}>
          <span className="d-none d-sm-inline">Start</span>
          <svg className="btn__svg-icon db-svg-icon db-svg-icon--play" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M8 6.8v10.4a1 1 0 001.5.8l8.2-5.2a1 1 0 000-1.7L9.5 6a1 1 0 00-1.5.8z"/></svg>
        </div>
        <div className="db-bot__infotext"/>
      </div>

      <Progressbar value={66} />

      <div className="db-bot__form">
        <div className="db-bot__alert text-danger">{ errors }</div>
        <form>
          <div className="form-inline mx-4">
            <div className="form-group mr-2">
              <select
                value={type}
                onChange={e => setType(e.target.value)}
                className="form-control db-select--buy-sell"
              >
                <option value="market_buy">{I18n.t('bots.buy')}</option>
                <option value="market_sell">{I18n.t('bots.sell')}</option>
                <option value="limit_buy" disabled={!showLimitOrders}>{I18n.t('bots.limit_buy')}</option>
                <option value="limit_sell" disabled={!showLimitOrders}>{I18n.t('bots.limit_sell')}</option>
                }
              </select>
            </div>
            <div className="form-group mr-2">
              <select
                value={base}
                onChange={e => setBase(e.target.value)}
                className="form-control"
              >
                {
                  BASES.map(c =>
                    (<option key={c} value={c}>{renameSymbol(c)}</option>)
                  )
                }
                {
                  OTHER_BASES.map(c =>
                    (<option key={c} value={c} disabled={true}>{renameSymbol(c)}</option>)
                  )
                }
              </select>
            </div>
            <div className="form-group mr-2">{I18n.t('bots.for')}</div>
            <div className="form-group mr-2">
              <input
                type="tel"
                min="1"
                value={price}
                onChange={e => setPrice(e.target.value)}
                className="form-control db-input--dca-amount"
              />
            </div>
            <div className="form-group mr-2">
              <select
                value={quote}
                onChange={e => setQuote(e.target.value)}
                className="form-control"
              >
                {
                  validQuotesForSelectedBase().map(c =>
                    (<option key={c} value={c}>{renameSymbol(c)}</option>)
                  )
                }
              </select>
            </div>
            <div className="form-group mr-2">/</div>
            <div className="form-group mr-2">
              <select
                value={interval}
                onChange={e => setInterval(e.target.value)}
                className="form-control"
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
              onChange={() => setForceSmartIntervals(!forceSmartIntervals)}
              className="mr-2" />
            <span>{I18n.t('bots.force_smart_intervals')}</span>
          </label>
        </form>
        {isLimitOrder() &&
        <span className="db-limit-bot-modifier">
          { isSellOffer() ? I18n.t('bots.sell') : I18n.t('bots.buy') } <input
            type="text"
            min="0"
            step="0.1"
            className="form-control"
            onChange={e => setPercentage(e.target.value)}
            placeholder="0"
            /> % { isSellOffer() ? I18n.t('bots.above') : I18n.t('bots.below') } {I18n.t('bots.price')}.<sup>*</sup></span> }
      </div>
      {isLimitOrder() && <LimitOrderNotice />}
      <div className="db-bot__footer">
        <ResetButton />
      </div>
    </div>
  )
}
