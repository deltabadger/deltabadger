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
    <div>
      { errors }
      <form onSubmit={_handleSubmit}>
        <label>
          API Key:
          <input
            type="text"
            value={key}
            onChange={e => setKey(e.target.value)}
          />
        </label>
        <label>
          Secret API Key:
          <input
            type="text"
            value={secret}
            onChange={e => setSecret(e.target.value)}
          />
        </label>
        <input type="submit" value="Submit" />
      </form>
      <ResetButton />
    </div>
  )
}
