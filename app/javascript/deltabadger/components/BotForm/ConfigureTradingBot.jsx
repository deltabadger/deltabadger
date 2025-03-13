import React, {useEffect, useRef, useState} from 'react'
import {Breadcrumbs} from './Breadcrumbs'
import {Progressbar} from './Progressbar'
import LimitOrderNotice from "./LimitOrderNotice";
import {getSpecialSymbols, renameCurrency, renameSymbol, shouldRename, shouldShowSubaccounts} from "../../utils/symbols";
import I18n from "i18n-js";
import {RawHTML} from "../RawHtml";
import API from "../../lib/API";
import {StartButton} from "../buttons";

export const ConfigureTradingBot = ({ showLimitOrders, currentExchange, handleReset, handleSubmit, handleSmartIntervalsInfo, setShowInfo, disable, errors }) => {
  const shouldRenameSymbols = shouldRename(currentExchange.name)
  const [showSubaccounts,setShowSubaccounts] = useState(false)

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
  const [percentage, setPercentage] = useState("0.1");
  const [forceSmartIntervals, setForceSmartIntervals] = useState(false);
  const [useSubaccount,setUseSubaccounts] = useState(false)
  const [selectedSubaccount, setSelectedSubaccount] = useState('')
  const [subaccountsList, setSubaccountsList] = useState([])
  const [smartIntervalsValue, setSmartIntervalsValue] = useState("0");
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
      className="button button--link"
    >
      <i className="material-icons">close</i>
      <span>Cancel</span>
    </div>
  )

  const StartButton = () => {
    const [isOpen, setOpen] = useState(false)
    const node = useRef()
    const [newIntervalsValue, setNewIntervalsValue] = useState("1");
    const [frequencyLimit, setFrequencyLimit] = useState("100");

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
                }} className="button button--primary button--outline">{I18n.t('bots.setup.frequency_limit.back_to_settings')}
                </div>
                <div onClick={
                  _handleSmartIntervalsChange
                } className="button button--success">{I18n.t('bots.setup.frequency_limit.start_the_bot')}
                </div>
              </div>
            </div>
          </div>
      )
    }
    const _handleSmartIntervalsChange = (evt) => {
      setOpen(false)
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
      let frequencyLimitExceeded = false
      let frequencyResponse = null
      try {
        frequencyResponse = await API.checkFrequencyExceed(frequencyParams)
        frequencyLimitExceeded = frequencyResponse['limit_exceeded']
        setFrequencyLimit(frequencyResponse['frequency_limit'])
        setNewIntervalsValue(frequencyResponse['new_intervals_value'].toString());
        if (frequencyLimitExceeded) {
          setOpen(true);
        } else {
          _handleSubmit(evt, smartIntervalsValue)
        }
      } catch (e) {
        console.error(e)
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
              className={`button ${disableSubmit ? 'button--outline button--disabled' : 'button--success'}`}>
            <div className="animicon animicon--start">
              <div className="animicon__a"></div>
              <div className="animicon__b"></div>
            </div>
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
      botType: 'trading',
    }

    return botParams
  }

  const setSubaccounts = async () => {
    await API.getSubaccounts(currentExchange.id).then(data => {
            setSubaccountsList(data.data['subaccounts']);
            setShowSubaccounts(data.data['subaccounts'].length > 0 && shouldShowSubaccounts(currentExchange.name));
            setSelectedSubaccount(data.data['subaccounts'].length > 0 ? data.data['subaccounts'][0] : '');
          });
  }

  useEffect(() => {
    async function fetchSmartIntervalsInfo()  {
      const data = await handleSmartIntervalsInfo(getBotParams())
      if (data.data.minimum === undefined)
        return;

      if (isLimitOrderDefinedInBase(currentExchange.name) && isLimitOrder()) {
        data.data.minimum = data.data.minimum_limit
        data.data.side = 'base'
      }

      const minimum = data.data.minimum
      const currency = data.data.side === 'base' ? renameCurrency(base, currentExchange.name) : renameCurrency(quote, currentExchange.name)
      await setSubaccounts()

      setMinimumOrderParams(getMinimumOrderParams(data))
      setPercentage(data.data.fee)
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
      botType: 'trading',
      priceRangeEnabled,
      priceRange,
      useSubaccount,
      selectedSubaccount
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
        <Breadcrumbs step={3} />
        <StartButton/>
      </div>

      <Progressbar value={66} />

      <div className="db-bot__form">
        <div className="db-bot__alert text-danger">{ errors }</div>
        <form>

          <div className="form-inline db-bot__form__schedule">
            <div className="form-group mr-2">
              <select
                value={type}
                onChange={e => setType(e.target.value)}
                className="bot-input bot-input--select bot-input--order-type bot-input--paper-bg"
              >
                <option value="market_buy">{I18n.t('bots.buy')}</option>
                <option value="market_sell">{I18n.t('bots.sell')}</option>
                <option value="limit_buy" disabled={!showLimitOrders}>{I18n.t('bots.limit_buy')}</option>
                <option value="limit_sell" disabled={!showLimitOrders}>{I18n.t('bots.limit_sell')}</option>
              </select>
            </div>
            {isSellOffer()?
                <>
                  <div className="form-group mr-2">
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
                  <div className="form-group mr-2">{I18n.t('bots.for')}</div>
                </>
                :
                <>
                  <div className="form-group mr-2">
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
                  <div className="form-group mr-2">{I18n.t('bots.for')}</div>
                  <div className="form-group mr-2">
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

          {showSubaccounts && <label
              className="alert alert-primary"
              disabled={!useSubaccount}
          >
            <input
                type="checkbox"
                checked={useSubaccount}
                onChange={() => setUseSubaccounts(!useSubaccount)}
            />
            <div>
              <RawHTML tag="span">{splitTranslation(I18n.t('bots.subaccounts_info'))}</RawHTML>
              <select
                  value={selectedSubaccount}
                  onChange={e => setSelectedSubaccount(e.target.value)}
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
                  {getSmartIntervalsDisclaimer()}
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
              <RawHTML tag="span">{ I18n.t('bots.feecutter_html')}</RawHTML> <input
                type="text"
                value={percentage}
                size={(percentage.length > 0) ? percentage.length : 3}
                className="bot-input bot-input--sizable"
                onChange={e => setPercentage(e.target.value)}
                onBlur={validatePercentage}
                disabled={!showLimitOrders || !isLimitOrder()}
                /> % { isSellOffer() ? I18n.t('bots.above') : I18n.t('bots.below') } {I18n.t('bots.price')}.

              { isLimitOrder() && <small><LimitOrderNotice /></small> }
              { !showLimitOrders && <div className="bot input bot-input--pro-plan-only--before"><a href={`/${document.body.dataset.locale}/upgrade`} >Pro</a></div> }
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
              <RawHTML tag="span">{splitTranslation(I18n.t(isSellOffer() ? 'bots.price_range_sell_html' :'bots.price_range_buy_html', {quote: quote, base: base}))[0]}</RawHTML>
              <input
                type="text"
                className="bot-input bot-input--sizable"
                value={priceRange.low}
                onChange={e => setPriceRange({low: e.target.value, high: priceRange.high})}
                disabled={!showLimitOrders}
                size={Math.max(priceRange.low.length, 1)}
              />

              <RawHTML tag="span">{splitTranslation(I18n.t(isSellOffer() ? 'bots.price_range_sell_html' :'bots.price_range_buy_html', {quote: quote, base: base}))[1]}</RawHTML>
              <input
                type="text"
                className="bot-input bot-input--sizable"
                value={priceRange.high}
                onChange={e => setPriceRange({low: priceRange.low, high: e.target.value})}
                disabled={!showLimitOrders}
                size={ Math.max(priceRange.high.length, 1) }
              />
              <RawHTML tag="span">{splitTranslation(I18n.t(isSellOffer() ? 'bots.price_range_sell_html' :'bots.price_range_buy_html', {quote: quote, base: base}))[2]}</RawHTML>
              { !showLimitOrders && <div className="bot input bot-input--pro-plan-only--before"><a href={`/${document.body.dataset.locale}/upgrade`} >Pro</a></div> }
            </div>
          </label>

        </form>

      </div>

      <div className="bot-footer">
        <ResetButton />
      </div>
    </div>
  )
}
