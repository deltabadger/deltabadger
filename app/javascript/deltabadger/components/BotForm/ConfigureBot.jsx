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
        <div className="db-bot__infotext--setup">Set the schedule.</div>
        <div onClick={_handleSubmit} className={`btn ${disableSubmit ? 'btn-outline-secondary disabled' : 'btn-outline-success'}`}>
          <span className="d-none d-sm-inline">Start</span><i className="material-icons-round">play_arrow</i>
        </div>
        <div className="db-bot__infotext">
          <div className="progress progress--thin progress--bot-setup">
            <div className="progress-bar" role="progressbar" style={{width: "66%", ariaValuenow: "66", ariaValuemin: "0", ariaValuemax: "100"}}></div>
          </div>
        </div>
      </div>

      <div className="row db-bot__form">
        <div className="db-bot__alert text-danger">{ errors }</div>
        <form className="form-inline mx-4">
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
          <div className="form-group mr-2">
            <select
              className="form-control"
              disabled={true}
            >
              <option value="buy">BTC</option>
            </select>
          </div>
          <div className="form-group mr-2">for</div>
          <div className="form-group mr-2">
            <input
              type="text"
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
      <ResetButton />
    </div>
  )
}
