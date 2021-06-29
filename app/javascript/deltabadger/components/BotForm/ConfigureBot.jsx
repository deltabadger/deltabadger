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

  const [type, setType] = useState("market_buy");
  const [price, setPrice] = useState("");
  const [base, setBase] = useState(BASES[0]);
  const [quote, setQuote] = useState(QUOTES[0]);
  const [minimumOrderParams, setMinimumOrderParams] = useState({});
  const [interval, setInterval] = useState("hour");
  const [percentage, setPercentage] = useState("0");
  const [forceSmartIntervals, setForceSmartIntervals] = useState(false);
  const [smartIntervalsValue, setSmartIntervalsValue] = useState(0.0);
  const [currencyOfMinimum, setCurrencyOfMinimum] = useState(QUOTES[0]);
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
  };

  useEffect(() => {
    document.addEventListener("mousedown", handleClickOutside);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
    };
  }, []);

  const disableSubmit = disable || price.trim() === ''

  const getMinimumOrderParams = (data) => {
    const minimumOrderParams = {
      value: data.data.minimum >= 1 ? Math.floor(data.data.minimum) : data.data.minimum,
      currency: data.data.side === 'base' ? renameCurrency(base, currentExchange.name) : renameCurrency(quote, currentExchange.name),
      showQuote: data.data.side === 'base',
      quoteValue: data.data.minimumQuote
    }

    return minimumOrderParams
  }

  const getBotParams = () => {
    const botParams = {
      type,
      base,
      quote,
      interval,
      forceSmartIntervals,
      smartIntervalsValue,
      price: price.trim(),
      percentage: isLimitOrder() ? percentage.trim() : undefined,
      botType: 'free',
    }

    return botParams
  }

  useEffect(() => {
    async function fetchSmartIntervalsInfo()  {
      const data = await handleSmartIntervalsInfo(getBotParams())
      const minimum = data.data.minimum >= 1 ? Math.floor(data.data.minimum) : data.data.minimum
      const currency = data.data.side === 'base' ? renameCurrency(base, currentExchange.name) : renameCurrency(quote, currentExchange.name)

      setMinimumOrderParams(getMinimumOrderParams(data))
      setSmartIntervalsValue(minimum)
      setCurrencyOfMinimum(currency)
    }

    fetchSmartIntervalsInfo()
  }, []);

  const _handleSubmit = (evt) => {
    evt.preventDefault();
    console.log(smartIntervalsValue)
    const botParams = {
      type,
      base,
      quote,
      interval,
      forceSmartIntervals,
      smartIntervalsValue,
      price: price.trim(),
      percentage: isLimitOrder() ? percentage.trim() : undefined,
      botType: 'free',
    }
    !disableSubmit && handleSubmit(botParams);
  }

  const isLimitOrder = () => type === 'limit_buy' || type === 'limit_sell'

  const isSellOffer = () => type === 'market_sell' || type === 'limit_sell'

  const splitTranslation = (s) => {
    return s.split(/<split>.*<\/split>/)
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
            <div className="form-group mr-3">
              <select
                value={type}
                onChange={e => setType(e.target.value)}
                className="bot-input bot-input--select bot-input--order-type bot-input--paper-bg"
              >
                <option value="market_buy">{I18n.t('bots.buy')}</option>
                <option value="market_sell">{I18n.t('bots.sell')}</option>
                <option value="limit_buy" disabled={!showLimitOrders}>{I18n.t('bots.limit_buy')}</option>
                <option value="limit_sell" disabled={!showLimitOrders}>{I18n.t('bots.limit_sell')}</option>
                }
              </select>
            </div>
            <div className="form-group mr-3">
              <select
                value={base}
                onChange={e => setBase(e.target.value)}
                className="bot-input bot-input--select bot-input--ticker bot-input--paper-bg"
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
            <div className="form-group mr-3">{I18n.t('bots.for')}</div>
            <div className="form-group mr-3">
              <input
                type="tel"
                min="1"
                size={(price.length > 0) ? price.length : 3 }
                value={price}
                onChange={e => setPrice(e.target.value)}
                className="bot-input bot-input--sizable bot-input--paper-bg"
              />
            </div>
            <div className="form-group mr-2">
              <select
                value={quote}
                onChange={e => setQuote(e.target.value)}
                className="bot-input bot-input--select bot-input--ticker bot-input--paper-bg"
              >
                {
                  validQuotesForSelectedBase().map(c =>
                    (<option key={c} value={c}>{renameSymbol(c)}</option>)
                  )
                }
              </select>
            </div>
            <div className="form-group mr-2">/</div>
            <div className="form-group">
              <select value={interval}
                      onChange={e => setInterval(e.target.value)}
                      className="bot-input bot-input--select bot-input--interval bot-input--paper-bg"
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
              checked={forceSmartIntervals}
              onChange={() => setForceSmartIntervals(!forceSmartIntervals)}
            />
            <div>
            <RawHTML tag="span">{splitTranslation(I18n.t('bots.force_smart_intervals_html', {currency: currencyOfMinimum}))[0]}</RawHTML>
              <input
                type="tel"
                className="bot-input bot-input--sizable hide-when-disabled"
                value={smartIntervalsValue}
                onChange={e => setSmartIntervalsValue(e.target.value)}
                size={(smartIntervalsValue.length > 0) ? smartIntervalsValue.length : 3 }
                min={minimumOrderParams.value}
              />
              <RawHTML tag="span">{splitTranslation(I18n.t('bots.force_smart_intervals_html', {currency: currencyOfMinimum}))[1]}</RawHTML>
            </div>

            <small className="hide-when-running hide-when-disabled">
              <div>
                <sup>*</sup>Orders size on {currentExchange.name} is defined in {currencyOfMinimum}, and the minimum size is {minimumOrderParams.value}.
              </div>
            </small>
          </label>

          {isLimitOrder() &&

          <label
            className="alert alert-primary"
          >
            <div>

              { isSellOffer() ? I18n.t('bots.sell') : I18n.t('bots.buy') } <input
                type="tel"
                size={(percentage.length > 0) ? percentage.length : 3 }
                value={percentage}
                className="bot-input bot-input--sizable"
                onChange={e => setPercentage(e.target.value)}
                /> % { isSellOffer() ? I18n.t('bots.above') : I18n.t('bots.below') } {I18n.t('bots.price')}.<sup>*</sup>

              {isLimitOrder() && <small><LimitOrderNotice /></small>}

            </div>

          </label> }

        </form>

      </div>

      <div className="db-bot__footer">
        <ResetButton />
      </div>
    </div>
  )
}
