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
      className="btn btn-link btn--reset"
    >
      Reset<i className="fas fa-redo ml-1"></i>
    </div>
  )

  const disableSubmit = key == '' || secret == ''

  const _handleSubmit = (evt) => {
      evt.preventDefault();
      !disableSubmit && handleSubmit(key, secret)
  }

  return (
    <div className="db-bots__item db-bot db-bot--get-apikey">
      <div className="db-bot__header">
        <div onClick={_handleSubmit} className={`btn btn-primary ${disableSubmit ? 'disabled' : ''}`}><span>Submit</span> <i className="fas fa-arrow-right"></i></div>
        <div className="db-bot__infotext db-bot__infotext--setup">Get API Key (2 of 3)
          <div className="progress progress--thin progress--bot-setup">
            <div className="progress-bar" role="progressbar" style={{width: "33%", ariaValuenow: "33", ariaValuemin: "0", ariaValuemax: "100"}}></div>
          </div>
        </div>

      </div>
      <div className="row db-bot__exchanges">
        { errors }
        <form onSubmit={_handleSubmit} className="form-row w-100 mx-0 mt-4">
          <div className="col form-group db-form-group--fg-2">
            <label>API Key:</label>
            <input
              type="text"
              value={key}
              onChange={e => setKey(e.target.value)}
              className="form-control"
            />
          </div>
          <div className="col form-group db-form-group--fg-2">
            <label>Secret API Key:</label>
            <input
              type="text"
              value={secret}
              onChange={e => setSecret(e.target.value)}
              className="form-control col"
            />
          </div>
        </form>
      </div>
      <Instructions exchangeName={pickedExchangeName} />
      <ResetButton />
    </div>
  )
}
