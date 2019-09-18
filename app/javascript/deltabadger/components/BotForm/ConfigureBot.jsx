import React, { useState } from 'react'

export const ConfigureBot = ({ handleReset, handleSubmit }) => {
  const [type, setType] = useState("sell");
  const [price, setPrice] = useState("");
  const [currency, setCurrency] = useState("USD");
  const [interval, setInterval] = useState("month");

  const ResetButton = () => (
    <div
      onClick={() => handleReset()}
      className="btn btn-link btn--reset"
    >
      Reset<i className="fas fa-redo ml-1"></i>
    </div>
  )

  const _handleSubmit = (evt) => {
      evt.preventDefault();
      const botParams = { type, price, currency, interval}
      handleSubmit(botParams);
  }

  return (
    <div className="db-bots__item db-bot db-bot--dca db-bot--ready">
      <div className="db-bot__header">
        <div onClick={_handleSubmit} className="btn btn-success"><span>Start</span> <i className="fas fa-play"></i></div>
        <div className="db-bot__infotext">
          <div className="db-bot__infotext__left">
            Bitbay:BTCPLN EXchange name
          </div>
          <div className="db-bot__infotext__right">
            Ready to go!
          </div>
          <div className="progress progress--thin progress--bot-setup">
            <div className="progress-bar bg-success" role="progressbar" style={{width: "0%", ariaValuenow: "25", ariaValuemin: "0", ariaValuemax: "100"}}></div>
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
            placeholder="10"
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
              <option value="hour">hour</option>
              <option value="day">day</option>
              <option value="week">week</option>
              <option value="minutes">1 minutes</option>
            </select>
          </div>
        </form>
      </div>

      <ResetButton />
    </div>
  )
}
