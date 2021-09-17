import React, {useEffect, useRef, useState} from 'react'
import {Breadcrumbs} from './Breadcrumbs'
import {Progressbar} from './Progressbar'
import LimitOrderNotice from "./LimitOrderNotice";
import {getSpecialSymbols, renameCurrency, renameSymbol, shouldRename} from "../../utils/symbols";
import I18n from "i18n-js";
import {RawHTML} from "../RawHtml";
import API from "../../lib/API";
import {StartButton} from "../buttons";

export const ConfigureBot = ({ showLimitOrders, currentExchange, handleReset, handleSubmit, handleSmartIntervalsInfo, setShowInfo, disable, errors, frequencyLimit }) => {
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
  const [percentage, setPercentage] = useState("0.0");
  const [forceSmartIntervals, setForceSmartIntervals] = useState(false);
  const [smartIntervalsValue, setSmartIntervalsValue] = useState("0");
  const [newIntervalsValue, setNewIntervalsValue] = useState("1");
  const [currencyOfMinimum, setCurrencyOfMinimum] = useState(QUOTES[0]);
  const [priceRangeEnabled, setPriceRangeEnabled] = useState(false)
  const [priceRange, setPriceRange] = useState({ low: '0', high: '0' })
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

  const StartButton = () => {
    const [isOpen, setOpen] = useState(false)
    const node = useRef()

    const handleClickOutside = e => {
      if (node.current && node.current.contains(e.target)) {
        return;
      }
      setOpen(false)
    };
    const SmarterStartButtons = () => {
      return (
          <div>
            <div>
              <RawHTML tag="p">{I18n.t('bots.setup.frequency_limit.limit_exceeded', {frequency_limit: frequencyLimit, price: newIntervalsValue, currency: currencyOfMinimum})}</RawHTML>
              <div className="db-bot__modal__btn-group">
                <div onClick={() => {
                  setOpen(false)
                }} className="btn btn-outline-primary">{I18n.t('bots.setup.frequency_limit.back_to_settings')}
                </div>
                <div onClick={
                  _handleSmartIntervalsChange
                } className="btn btn-success">{I18n.t('bots.setup.frequency_limit.back_to_settings')}
                </div>
              </div>
            </div>
          </div>
      )
    }
    const _handleSmartIntervalsChange = (evt) => {
      console.log('Value: ', forceSmartIntervals)
      _handleSubmit(evt, newIntervalsValue)
    }
    const _handleStarts = async (evt) => {
      evt.preventDefault();
      const frequencyParams = {
        type,
        base,
        quote,
        interval,
        forceSmartIntervals,
        smartIntervalsValue,
        price: price.trim(),
        exchange_id: currentExchange.id,
        currency_of_minimum: currencyOfMinimum
      }
      let frequency_limit_exceeded = false
      let frequency_limit = null
      try {
        frequency_limit = await API.checkFrequencyExceed(frequencyParams)
        frequency_limit_exceeded = frequency_limit['limit_exceeded']
      } catch (e) {
        console.error(e)
      } finally {
        if (frequency_limit_exceeded) {
          setNewIntervalsValue(frequency_limit['new_intervals_value'])
          console.log(newIntervalsValue)
          setOpen(true);
        } else {
          _handleSubmit(evt, smartIntervalsValue)
        }
      }
    }

    useEffect(() => {
      document.addEventListener("mousedown", handleClickOutside);
      return () => {
        document.removeEventListener("mousedown", handleClickOutside);
      };
    }, []);

    return(
        <div>
          <div
              onClick={_handleStarts}
              className={`btn ${disableSubmit ? 'btn-outline-secondary disabled' : 'btn-outline-success'}`}>
            <span className="d-none d-sm-inline">Start</span>
            <svg className="btn__svg-icon db-svg-icon db-svg-icon--play" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
              <path d="M8 6.8v10.4a1 1 0 001.5.8l8.2-5.2a1 1 0 000-1.7L9.5 6a1 1 0 00-1.5.8z"/>
            </svg>
          </div>
          { isOpen &&
          <div ref={node} className="db-bot__modal">
            <div className="db-bot__modal__content">
              <SmarterStartButtons />
            </div>
          </div>
          }
        </div>
    )
  }

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
      value: data.data.minimum,
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
      if (isLimitOrderDefinedInBase(currentExchange.name) && isLimitOrder()) {
        data.data.minimum = data.data.minimum_limit
        data.data.side = 'base'
      }

      const minimum = data.data.minimum
      const currency = data.data.side === 'base' ? renameCurrency(base, currentExchange.name) : renameCurrency(quote, currentExchange.name)

      setMinimumOrderParams(getMinimumOrderParams(data))
      setSmartIntervalsValue(minimum.toString())
      setCurrencyOfMinimum(currency)
    }

    fetchSmartIntervalsInfo()
  }, [base, quote, type]);

  const validateSmartIntervalsValue = () => {
    if (isNaN(smartIntervalsValue) || smartIntervalsValue < minimumOrderParams.value){
      setSmartIntervalsValue(minimumOrderParams.value)
    }
  }

  const validatePercentage = () => {
    if (isNaN(percentage) || percentage < 0){
      setPercentage('0')
    }
  }

  const _handleSubmit = (evt, smartIntervalsValue) => {
    evt.preventDefault();
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
      priceRangeEnabled,
      priceRange
    }
    !disableSubmit && handleSubmit(botParams);
  }

  const isLimitOrder = () => type === 'limit_buy' || type === 'limit_sell'

  const isSellOffer = () => type === 'market_sell' || type === 'limit_sell'

  const setLimitOrderCheckbox = () => {
    if (isLimitOrder()) {
      isSellOffer() ? setType('market_sell') : setType('market_buy')
    } else {
      isSellOffer() ? setType('limit_sell') : setType('limit_buy')
    }
  }

  const isLimitOrderDefinedInBase = (name) => ['Coinbase Pro', 'KuCoin'].includes(name)

  const splitTranslation = (s) => {
    return s.split(/<split>.*?<\/split>/)
  }

  const getSmartIntervalsDisclaimer = () => {
    if (minimumOrderParams.showQuote) {
      return I18n.t('bots.smart_intervals_disclaimer', {exchange: currentExchange.name, currency: currencyOfMinimum, minimum: minimumOrderParams.value})
    } else {
      return I18n.t('bots.smart_intervals_disclaimer_quote', {currency: currencyOfMinimum, minimum: minimumOrderParams.value});
    }
  }

  return (
    <div className="db-bots__item db-bot db-bot--dca db-bot--setup db-bot--ready db-bot--active">

      <div className="db-bot__header">
        <Breadcrumbs step={2} />
        <StartButton/>
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
            {isSellOffer()?
                <>
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
                </>
                :
                <>
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
                        type="text"
                        min="1"
                        size={(price.length > 0) ? price.length : 3 }
                        value={price}
                        onChange={e => setPrice(e.target.value)}
                        className="bot-input bot-input--sizable bot-input--paper-bg"
                    />
                  </div>
                </>

            }
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
                type="text"
                size={(smartIntervalsValue.length > 0) ? smartIntervalsValue.length : 3 }
                className="bot-input bot-input--sizable"
                value={smartIntervalsValue}
                onChange={e => setSmartIntervalsValue(e.target.value)}
                onBlur={validateSmartIntervalsValue}
                min={minimumOrderParams.value}
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
            disabled={!showLimitOrders || !isLimitOrder()}
          >
            <input
              type="checkbox"
              checked={isLimitOrder()}
              onChange={setLimitOrderCheckbox}
              disabled={!showLimitOrders}
            />
            <div>
              { isSellOffer() ? I18n.t('bots.sell') : I18n.t('bots.buy') } <input
                type="text"
                size={(percentage.length > 0) ? percentage.length : 3 }
                value={percentage}
                className="bot-input bot-input--sizable"
                onChange={e => setPercentage(e.target.value)}
                onBlur={validatePercentage}
                disabled={!showLimitOrders || !isLimitOrder()}
                /> % { isSellOffer() ? I18n.t('bots.above') : I18n.t('bots.below') } {I18n.t('bots.price')}.<sup>*</sup>

              { isLimitOrder() && <small><LimitOrderNotice /></small> }
              { !showLimitOrders && <a href={`/${document.body.dataset.locale}/upgrade`} className="bot input bot-input--hodler-only--before">Hodler only</a> }
            </div>
          </label>

          <label
            className="alert alert-primary"
            disabled={!showLimitOrders || !priceRangeEnabled}
          >
            <input
              type="checkbox"
              checked={priceRangeEnabled}
              onChange={() => setPriceRangeEnabled(!priceRangeEnabled)}
              disabled={!showLimitOrders}
            />
            <div>
              <RawHTML tag="span">{splitTranslation(I18n.t(isSellOffer() ? 'bots.price_range_sell_html' :'bots.price_range_buy_html', {currency: quote}))[0]}</RawHTML>
              <input
                type="text"
                className="bot-input bot-input--sizable"
                value={priceRange.low}
                onChange={e => setPriceRange({low: e.target.value, high: priceRange.high})}
                disabled={!showLimitOrders}
                size={Math.max(priceRange.low.length, 1)}
              />

              <RawHTML tag="span">{splitTranslation(I18n.t(isSellOffer() ? 'bots.price_range_sell_html' :'bots.price_range_buy_html', {currency: quote}))[1]}</RawHTML>
              <input
                type="text"
                className="bot-input bot-input--sizable"
                value={priceRange.high}
                onChange={e => setPriceRange({low: priceRange.low, high: e.target.value})}
                disabled={!showLimitOrders}
                size={ Math.max(priceRange.high.length, 1) }
              />
              <RawHTML tag="span">{splitTranslation(I18n.t(isSellOffer() ? 'bots.price_range_sell_html' :'bots.price_range_buy_html', {currency: quote}))[2]}</RawHTML>
              { !showLimitOrders && <a href={`/${document.body.dataset.locale}/upgrade`} className="bot input bot-input--hodler-only--before">Hodler only</a> }
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
