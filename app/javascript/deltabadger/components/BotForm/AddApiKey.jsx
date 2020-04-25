import React, { useState } from 'react'
import { Instructions } from './AddApiKey/Instructions';

export const AddApiKey = ({
  pickedExchangeName,
  handleReset,
  handleSubmit,
  errors
}) => {
  const [key, setKey] = useState("");
  const [secret, setSecret] = useState("");

  const ResetButton = () => (
    <div
      onClick={() => handleReset()}
      className="btn btn-link btn--reset btn--reset-back"
    >
      <i className="material-icons-round">close</i>
      <span>Cancel</span>
    </div>
  )

  const disableSubmit = key == '' || secret == ''

  const _handleSubmit = (evt) => {
      evt.preventDefault();
      !disableSubmit && handleSubmit(key, secret)
  }

  return (
    <div className="db-bots__item db-bot db-bot--get-apikey db-bot--active">
      <div className="db-bot__header">
        <div className="db-bot__infotext--setup">Get API Keys</div>
        <div onClick={_handleSubmit} className={`btn ${disableSubmit ? 'btn-outline-secondary disabled' : 'btn-outline-primary'}`}><span>Next</span> <i className="material-icons-round">arrow_forward</i></div>
        <div className="db-bot__infotext">
          <div className="progress progress--thin progress--bot-setup">
            <div className="progress-bar" role="progressbar" style={{width: "33%", ariaValuenow: "33", ariaValuemin: "0", ariaValuemax: "100"}}></div>
          </div>
        </div>

      </div>
      <div className="row db-bot__form db-bot__form--apikeys">
        <div className="db-bot__alert text-danger">{ errors }</div>
        <form onSubmit={_handleSubmit} className="form-row">
          <div className="col form-group">
            <label>API/Public Key:</label>
            <input
              type="text"
              value={key}
              onChange={e => setKey(e.target.value)}
              className="form-control"
            />
          </div>
          <div className="col form-group">
            <label>Private Key:</label>
            <input
              type="text"
              value={secret}
              onChange={e => setSecret(e.target.value)}
              className="form-control col"
            />
          </div>
        </form>
      </div>
      <div className="db-exchange-instructions">
        <div className="alert alert--trading-agreement">
          <p><b>Achtung!</b> If your Kraken account is verified with a German address, you will need to accept a <a href="https://support.kraken.com/hc/en-us/articles/360036157952" target="_blank" rel="noopener" title="Trading agreement">trading agreement</a> in order to place market and margin orders.</p>
          <div className="form-check">
            <input className="form-check-input" type="checkbox"></input>
            <label className="form-check-label"><b> I accept <a href="https://support.kraken.com/hc/en-us/articles/360036157952" target="_blank" rel="noopener" title="Trading agreement">trading agreement</a></b>.</label>
          </div>
        </div>
      </div>
      <Instructions exchangeName={pickedExchangeName} />
      <ResetButton />
    </div>
  )
}
