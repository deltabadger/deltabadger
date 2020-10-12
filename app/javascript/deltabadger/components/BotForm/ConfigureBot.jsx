import React, { useState } from 'react'
import LimitOrderNotice from "./LimitOrderNotice";

export const ConfigureBot = ({ showLimitOrders, currentExchange, handleReset, handleSubmit, disable, errors }) => {
  const [type, setType] = useState("market_buy");
  const [price, setPrice] = useState("");
  const [base, setBase] = useState(currentExchange.symbols[0].base);
  const [quote, setQuote] = useState(currentExchange.symbols[0].quote);
  const [interval, setInterval] = useState("hour");
  const [percentage, setPercentage] = useState("0");

  const ResetButton = () => (
    <div
      onClick={() => handleReset()}
      className="btn btn-link btn--reset btn--reset-back"
    >
      <i className="material-icons-round">close</i>
      <span>Cancel</span>
    </div>
  )

  const disableSubmit = disable || price.trim() === ''

  const _handleSubmit = (evt) => {
    evt.preventDefault();
    const botParams = {
      type,
      base,
      quote,
      interval,
      price: price.trim(),
      percentage: isLimitOrder() ? percentage.trim() : undefined,
      botType: 'free',
    }
    !disableSubmit && handleSubmit(botParams);
  }

  const isLimitOrder = () => type === 'limit_buy' || type === 'limit_sell'

  const isSellOffer = () => type === 'market_sell' || type === 'limit_sell'

  return (
    <div className="db-bots__item db-bot db-bot--dca db-bot--setup db-bot--ready db-bot--active">

      <div className="db-bot__header">
        <div className="db-bot__infotext--setup"><span className="db-breadcrumbs">Exchange &rarr; API Key &rarr; <em>Schedule</em></span></div>
        <div onClick={_handleSubmit} className={`btn ${disableSubmit ? 'btn-outline-secondary disabled' : 'btn-outline-success'}`}>
          <span className="d-none d-sm-inline">Start</span>
          <svg className="btn__svg-icon db-svg-icon db-svg-icon--play" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M8 6.8v10.4a1 1 0 001.5.8l8.2-5.2a1 1 0 000-1.7L9.5 6a1 1 0 00-1.5.8z"/></svg>
        </div>
        <div className="db-bot__infotext"/>
      </div>

      <div className="db-bot__progress progress progress--thin progress--bot-setup">
        <div className="progress-bar" role="progressbar" style={{width: "66%", ariaValuenow: "66", ariaValuemin: "0", ariaValuemax: "100"}}/>
      </div>

      <div className="db-bot__form">
        <div className="db-bot__alert text-danger">{ errors }</div>
        <form className="form-inline mx-4">
          <div className="form-group mr-2">
            <select
              value={type}
              onChange={e => setType(e.target.value)}
              className="form-control db-select--buy-sell"
              id="exampleFormControlSelect1"
            >
              <option value="market_buy">Buy</option>
              <option value="market_sell">Sell</option>
              <option value="limit_buy" disabled={!showLimitOrders}>Limit Buy</option>
              <option value="limit_sell" disabled={!showLimitOrders}>Limit Sell</option>
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
                [...new Set(currentExchange.symbols.map(s => s.base))].map(c =>
                  (<option key={c} value={c}>{c}</option>)
                )
              }
            </select>
          </div>
          <div className="form-group mr-2">for</div>
          <div className="form-group mr-2">
            <input
              type="text"
              min="1"
              value={price}
              onChange={e => setPrice(e.target.value)}
              className="form-control mr-2 db-input--dca-amount"
            />
          </div>
          <div className="form-group mr-2">
            <select
              value={quote}
              onChange={e => setQuote(e.target.value)}
              className="form-control"
              id="exampleFormControlSelect1"
            >
              {
                [... new Set(currentExchange.symbols.map(s => s.quote))].map(c =>
                  (<option key={c} value={c}>{c}</option>)
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
              id="exampleFormControlSelect1"
            >
              <option value="hour">Hour</option>
              <option value="day">Day</option>
              <option value="week">Week</option>
              <option value="month">Month</option>
            </select>
          </div>
        </form>
        {isLimitOrder() &&
        <span className="db-limit-bot-modifier">
          { isSellOffer() ? 'Sell' : 'Buy' } <input
            type="text"
            min="0"
            step="0.1"
            lang="en-150"
            className="form-control"
            onChange={e => setPercentage(e.target.value)}
            placeholder="0"
        /> % { isSellOffer() ? 'above' : 'below'} the price.<sup>*</sup></span> }
      </div>
      {isLimitOrder() && <LimitOrderNotice />}
      <div className="db-bot__footer">
        <ResetButton />
      </div>
    </div>
  )
}
