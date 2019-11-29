import React, { useState } from 'react'

export const ConfigureBot = ({ handleReset, handleSubmit }) => {
  const [type, setType] = useState("buy");
  const [price, setPrice] = useState("");
  const [currency, setCurrency] = useState("USD");
  const [interval, setInterval] = useState("hour");

  const ResetButton = () => (
    <div
      onClick={() => handleReset()}
      className="btn btn-link btn--reset btn--reset-back"
    >
      <i className="material-icons-round">arrow_back</i>
      <span>Back</span>
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
        <div onClick={_handleSubmit} className={`btn ${disableSubmit ? 'btn-outline-secondary disabled' : 'btn-success'}`}>{disableSubmit ? '' : <span>Start</span> }<i className="material-icons-round">{disableSubmit ? 'more_horiz' : 'play_arrow'}</i></div>
        <div className="db-bot__infotext db-bot__infotext--setup">Set the schedule
          <div className="progress progress--thin progress--bot-setup">
            <div className="progress-bar" role="progressbar" style={{width: "66%", ariaValuenow: "66", ariaValuemin: "0", ariaValuemax: "100"}}></div>
          </div>
        </div>
      </div>

      <div className="row db-bot--dca__config-free">
        <form className="form-inline">
          <div className="form-group mr-2">
            <select
              value={type}
              onChange={e => setType(e.target.value)}
              className="form-control"
              id="exampleFormControlSelect1"
            >
              <option value="buy">Buy</option>
              <option value="sell">Sell</option>
            </select>
          </div>
          <div className="form-group mr-2">for</div>
          <input
            type="text"
            value={price}
            onChange={e => setPrice(e.target.value)}
            className="form-control mr-1"
          />

          <div className="form-group mr-2">
            <select
              value={currency}
              onChange={e => setCurrency(e.target.value)}
              className="form-control"
              id="exampleFormControlSelect1"
            >
              <option value="USD">USD</option>
              <option value="EUR">EUR</option>
              <option value="PLN">PLN</option>
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
              <option value="minute">Minute</option>
              <option value="hour">Hour</option>
              <option value="day">Day</option>
              <option value="week">Week</option>
              <option value="month">Month</option>
            </select>
          </div>
        </form>
      </div>
      <ResetButton />
    </div>
  )
}
