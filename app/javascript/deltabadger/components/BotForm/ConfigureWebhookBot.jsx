import React, {useEffect, useRef, useState} from 'react'
import {Breadcrumbs} from './Breadcrumbs'
import {Progressbar} from './Progressbar'
import LimitOrderNotice from "./LimitOrderNotice";
import {getSpecialSymbols, renameCurrency, renameSymbol, shouldRename, shouldShowSubaccounts} from "../../utils/symbols";
import I18n from "i18n-js";
import {RawHTML} from "../RawHtml";
import API from "../../lib/API";
import {StartButton} from "../buttons";

export const ConfigureWebhookBot = ({ showLimitOrders, currentExchange, handleReset, handleSubmit, handleSmartIntervalsInfo, setShowInfo, disable, errors }) => {
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

  const [type, setType] = useState("buy");
  const [additionalType, setAdditionalType] = useState("sell");
  const [price, setPrice] = useState("");
  const [additionalPrice, setAdditionalPrice] = useState("");
  const [name, setName] = useState('');
  const [base, setBase] = useState(BASES[0]);
  const [quote, setQuote] = useState(QUOTES[0]);
  const [minimumOrderParams, setMinimumOrderParams] = useState({});
  const [interval, setInterval] = useState("hour");
  const [triggerPossibility, setTriggerPossibility] = useState("first_time");
  const [triggerUrl] = useState();
  const [additionalTypeEnabled, setAdditionalTypeEnabled] = useState(false);
  const [additionalTriggerUrl] = useState();
  const [percentage, setPercentage] = useState("0.0");
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
      className="btn btn-link btn--reset btn--reset-back"
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
                }} className="btn btn-outline-primary">{I18n.t('bots.setup.frequency_limit.back_to_settings')}
                </div>
                <div onClick={
                  _handleSmartIntervalsChange
                } className="btn btn-success">{I18n.t('bots.setup.frequency_limit.start_the_bot')}
                </div>
              </div>
            </div>
          </div>
      )
    }
    const _handleSmartIntervalsChange = (evt) => {
      setOpen(false)
      _handleSubmit(evt)
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
          _handleSubmit(evt)
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
      name,
      price: price.trim(),
      triggerPossibility,
      additionalTypeEnabled,
      additionalType,
      additionalPrice: additionalPrice.trim(),
      botType: 'webhook',
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

      // if (isLimitOrderDefinedInBase(currentExchange.name) && isLimitOrder()) {
      //   data.data.minimum = data.data.minimum_limit
      //   data.data.side = 'base'
      // }

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

  const _handleSubmit = (evt) => {
    evt.preventDefault();
    const botParams = {
      type,
      base,
      quote,
      name,
      price: price.trim(),
      triggerPossibility,
      additionalTypeEnabled,
      additionalType,
      additionalPrice: additionalPrice.trim(),
      botType: 'webhook',
    }
    !disableSubmit && handleSubmit(botParams);
  }

  const isBuyOffer = () => type === 'buy' || type === 'buy_all';
  const isSellOffer = () => type === 'sell' || type === 'sell_all';
  const isBuySellType = (type) => type === 'buy' || type === 'sell';

  return (
    <div className="db-bots__item db-bot db-bot--webhook db-bot--setup db-bot--ready db-bot--active">

      <div className="db-bot__header">
        <Breadcrumbs step={3} />
        <StartButton/>
      </div>

      <Progressbar value={66} />

      <div className="db-bot__form">
        <div className="db-bot__alert text-danger">{ errors }</div>
        <form>

          <div className="form-inline mb-4">
            <div className="form-group mr-2">{I18n.t('bots.name')}</div>
            <div className="form-group">
              <input
                  type="text"
                  min="5"
                  value={name}
                  onChange={e => setName(e.target.value)}
                  className="bot-input bot-input--sizable bot-input--paper-bg"
              />
            </div>
          </div>

          <div className="form-inline mb-4">
            <div className="form-group mr-2">
              <select
                value={type}
                onChange={e => setType(e.target.value)}
                className="bot-input bot-input--select bot-input--order-type bot-input--paper-bg"
              >
                <option value="buy">{I18n.t('bots.buy')}</option>
                <option value="buy_all">{I18n.t('bots.buy_all')}</option>
                <option value="sell">{I18n.t('bots.sell')}</option>
                <option value="sell_all">{I18n.t('bots.sell_all')}</option>
                }
              </select>
            </div>
            {isSellOffer()?
                <>
                  {isBuySellType(type) && <div className="form-group mr-2">
                    <input
                        type="tel"
                        min="1"
                        size={(price.length > 0) ? price.length : 3 }
                        value={price}
                        onChange={e => setPrice(e.target.value)}
                        className="bot-input bot-input--sizable bot-input--paper-bg"
                    />
                  </div>}
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
                  {isBuySellType(type) && <div className="form-group mr-2">
                    <input
                        type="text"
                        min="1"
                        size={(price.length > 0) ? price.length : 3 }
                        value={price}
                        onChange={e => setPrice(e.target.value)}
                        className="bot-input bot-input--sizable bot-input--paper-bg"
                    />
                  </div>}
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
          </div>

          <div className="form-inline mb-4">
            <div className="form-group mr-2">
              <select
                  value={triggerPossibility}
                  onChange={e => setTriggerPossibility(e.target.value)}
                  className="bot-input bot-input--select bot-input--interval bot-input--paper-bg"
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
                https://example.com/webhooks/{triggerUrl}
              </div>
            </>}
          </div>

          <div className="form-inline mb-4">
            <div className="form-group mr-2">{I18n.t('bots.additional_title')}</div>
            <div className="form-group mr-2">
              <input
                  type="checkbox"
                  checked={additionalTypeEnabled}
                  onChange={() => setAdditionalTypeEnabled(!additionalTypeEnabled)}
              />
            </div>
          </div>

          <div className="form-inline db-bot__form__schedule">
            <div className="form-group mr-2">
              <select
                  value={additionalType}
                  onChange={e => setAdditionalType(e.target.value)}
                  className="bot-input bot-input--select bot-input--order-type bot-input--paper-bg"
                  disabled={!additionalTypeEnabled}
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
            <div className="form-group mr-2">
              {isSellOffer()?
                  <>
                    <div className="form-group mr-2">{renameSymbol(base)}</div>
                    <div className="form-group mr-2">{I18n.t('bots.for')}</div>
                    {isBuySellType(additionalType) && <div className="form-group mr-2">
                      <input
                          type="text"
                          min="1"
                          size={(additionalPrice.length > 0) ? additionalPrice.length : 3 }
                          value={additionalPrice}
                          onChange={e => setAdditionalPrice(e.target.value)}
                          className="bot-input bot-input--sizable bot-input--paper-bg"
                          disabled={!additionalTypeEnabled}
                      />
                    </div>}
                    <div className="form-group mr-2">{renameSymbol(quote)}</div>
                    <div className="form-group mr-2">{I18n.t('bots.'+triggerPossibility)}</div>
                  </> : <>
                    {isBuySellType(additionalType) && <div className="form-group mr-2">
                      <input
                          type="text"
                          min="1"
                          size={(additionalPrice.length > 0) ? additionalPrice.length : 3 }
                          value={additionalPrice}
                          onChange={e => setAdditionalPrice(e.target.value)}
                          className="bot-input bot-input--sizable bot-input--paper-bg"
                          disabled={!additionalTypeEnabled}
                      />
                    </div>}
                    <div className="form-group mr-2">{renameSymbol(base)}</div>
                    <div className="form-group mr-2">{I18n.t('bots.for')}</div>
                    <div className="form-group mr-2">{renameSymbol(quote)}</div>
                    <div className="form-group mr-2">{I18n.t('bots.'+triggerPossibility)}</div>
                  </>
              }
              {additionalTriggerUrl && <>
                <div className="form-group mr-2">
                  {I18n.t('bots.triggered_title')}
                </div>
                <div className="form-group bot-input bot-input--sizable bot-input--paper-bg">
                  https://example.com/webhooks/{additionalTriggerUrl}
                </div>
              </>}
            </div>
          </div>

        </form>

      </div>

      <div className="db-bot__footer">
        <ResetButton />
      </div>
    </div>
  )
}
