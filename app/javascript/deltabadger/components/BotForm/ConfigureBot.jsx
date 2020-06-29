import React, { useState } from 'react'

export const ConfigureBot = ({ currentExchange, handleReset, handleSubmit, errors }) => {
  const [type, setType] = useState("buy");
  const [price, setPrice] = useState("");
  const [currency, setCurrency] = useState("USD");
  const [interval, setInterval] = useState("hour");

  const ResetButton = () => (
    <div
      onClick={() => handleReset()}
      className="btn btn-link btn--reset btn--reset-back"
    >
      <i className="material-icons-round">close</i>
      <span>Cancel</span>
    </div>
  )

  const disableSubmit = price.trim() == ''

  const _handleSubmit = (evt) => {
    evt.preventDefault();
    const botParams = { type, currency, interval, price: price.trim(), botType: 'free' }
    !disableSubmit && handleSubmit(botParams);
  }

  return (
    <div className="db-bots__item db-bot db-bot--dca db-bot--ready db-bot--active">

      <div className="db-bot__header">
        <div className="db-bot__infotext--setup"><span class="db-breadcrumbs">Exchange &rarr; API Key &rarr; <em>Schedule</em></span></div>
        <div onClick={_handleSubmit} className={`btn ${disableSubmit ? 'btn-outline-secondary disabled' : 'btn-outline-success'}`}>
          <span className="d-none d-sm-inline">Start</span>
          <svg className="btn__svg-icon db-svg-icon db-svg-icon--play" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M8 6.8v10.4a1 1 0 001.5.8l8.2-5.2a1 1 0 000-1.7L9.5 6a1 1 0 00-1.5.8z"/></svg>
        </div>
        <div className="db-bot__infotext"></div>
      </div>

      <div className="db-bot__progress progress progress--thin progress--bot-setup">
        <div className="progress-bar" role="progressbar" style={{width: "66%", ariaValuenow: "66", ariaValuemin: "0", ariaValuemax: "100"}}></div>
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
              <option value="buy">Buy</option>
              <option value="sell">Sell</option>
              <option value="limit_buy" disabled>Limit Buy</option>
              <option value="limit_sell" disabled>Limit Sell</option>
            </select>
          </div>
          <div className="form-group mr-2">
            <select
              className="form-control"
            >
              <option value="buy">BTC</option>
              <option value="buy" disabled>ETH</option>
            </select>
          </div>
          <div className="form-group mr-2">for</div>
          <div className="form-group mr-2">
            <input
              type="text"
              min="1"
              value={price}
              onChange={e => setPrice(e.target.value)}
              className="form-control mr-1"
            />
          </div>
          <div className="form-group mr-2">
            <select
              value={currency}
              onChange={e => setCurrency(e.target.value)}
              className="form-control"
              id="exampleFormControlSelect1"
            >
              {
                currentExchange.currencies.map(c =>
                  (<option value={c}>{c}</option>)
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

      </div>
      <div className="db-bot__footer">
        <ResetButton />
      </div>
    </div>
  )
}
