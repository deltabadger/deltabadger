import React, { useState } from 'react'

export const AddApiKey = ({ handleReset, handleSubmit, errors }) => {
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

  const _handleSubmit = (evt) => {
      evt.preventDefault();
      handleSubmit(key, secret)
  }

  return (
    <div className="db-bots__item db-bot db-bot--get-apikey">
      <div className="db-bot__header">
        <div onClick={_handleSubmit} className="btn btn-primary"><span>Submit</span> <i className="fas fa-arrow-right"></i></div>
        <div className="db-bot__infotext db-bot__infotext--setup">Get API Key (2 of 3)
          <div className="progress progress--thin progress--bot-setup">
            <div className="progress-bar" role="progressbar" style={{width: "33%", ariaValuenow: "33", ariaValuemin: "0", ariaValuemax: "100"}}></div>
          </div>
        </div>

      </div>
      <div className="row db-bot__exchanges">
        { errors }
        <form onSubmit={_handleSubmit} class="form-row w-100 mx-0 mt-4">
          <div class="col form-group db-form-group--fg-2">
            <label>API Key:</label>
            <input
              type="text"
              value={key}
              onChange={e => setKey(e.target.value)}
              class="form-control"
            />
          </div>
          <div class="col form-group db-form-group--fg-2">
            <label>Secret API Key:</label>
            <input
              type="text"
              value={secret}
              onChange={e => setSecret(e.target.value)}
              class="form-control col"
            />
          </div>
        </form>
      </div>
      <div class="row">
        <div class="alert alert-warning mx-0 mb-3 col" role="alert">
          <b class="alert-heading mb-2">How to get API keys:</b>
          <hr/>
          <ol>
            <li>Login to Kraken</li>
            <li>Go to Settings</li>
            <li>Get keys</li>
          </ol>
        </div>
      </div>
      <ResetButton />
    </div>
  )
}
