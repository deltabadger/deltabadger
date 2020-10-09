import React, { useState } from 'react'
import { Instructions } from './AddApiKey/Instructions';

const apiKeyNames = exchangeName => {
  switch (exchangeName.toLowerCase()) {
      case 'binance': return { private: 'Secret Key', public: 'API Key' };
      case 'bitbay': return { private: 'Private Key', public: 'Public Key' };
      case 'kraken': return { private: 'Private Key', public: 'API Key' };
      default: return { private: 'Private Key', public: 'Public Key' };
  }
}

export const AddApiKey = ({
  pickedExchangeName,
  handleReset,
  handleSubmit,
  errors
}) => {
  const [key, setKey] = useState("");
  const [secret, setSecret] = useState("");
  const [agreement, setAgreement] = useState(false)

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
      !disableSubmit && handleSubmit(key, secret, agreement)
  }

  const { public: key_label, private: secret_label } = apiKeyNames(pickedExchangeName);

  return (
    <div className="db-bots__item db-bot db-bot--get-apikey db-bot--active">
      <div className="db-bot__header">
        <div className="db-bot__infotext--setup"><span className="db-breadcrumbs">Exchange &rarr; <em>API Key</em> &rarr; Schedule</span></div>
        <div onClick={_handleSubmit} className={`btn ${disableSubmit ? 'btn-outline-secondary disabled' : 'btn-outline-primary'}`}>
          <span>Next</span>
          <svg className="db-bot__svg-icon db-svg-icon db-svg-icon--arrow-forward" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M5 13h11.2l-5 4.9a1 1 0 000 1.4c.5.4 1.1.4 1.5 0l6.6-6.6c.4-.4.4-1 0-1.4l-6.6-6.6a1 1 0 10-1.4 1.4l4.9 4.9H5c-.6 0-1 .5-1 1s.5 1 1 1z"/></svg>
        </div>
        <div className="db-bot__infotext">
        </div>
      </div>
      <div className="db-bot__progress progress progress--thin progress--bot-setup">
        <div className="progress-bar" role="progressbar" style={{width: "33%", ariaValuenow: "33", ariaValuemin: "0", ariaValuemax: "100"}}></div>
      </div>
      <div className="db-bot__form db-bot__form--apikeys">
        <div className="db-bot__alert text-danger">{ errors }</div>
        <form onSubmit={_handleSubmit} className="form-row">
          <div className="col form-group">
            <label>{ key_label }</label>
            <input
              type="text"
              value={key}
              onChange={e => setKey(e.target.value)}
              className="form-control"
            />
          </div>
          <div className="col form-group">
            <label>{ secret_label }</label>
            <input
              type="text"
              value={secret}
              onChange={e => setSecret(e.target.value)}
              className="form-control col"
            />
          </div>
        </form>
      </div>
      { pickedExchangeName == "Kraken" &&
        <div className="db-exchange-instructions">
          <div className="alert alert--trading-agreement">
            <p><b>Achtung!</b> If your Kraken account is verified with a German address, you will need to accept a <a href="https://support.kraken.com/hc/en-us/articles/360036157952" target="_blank" rel="noopener" title="Trading agreement">trading agreement</a> in order to place market and margin orders.</p>
            <div className="form-check">
              <input
                type="checkbox"
                checked={agreement}
                onChange={e => setAgreement(!agreement)}
                className="form-check-input"
              />
              <label className="form-check-label"><b> I accept <a href="https://support.kraken.com/hc/en-us/articles/360036157952" target="_blank" rel="noopener" title="Trading agreement">trading agreement</a></b>.</label>
            </div>
          </div>
        </div>
      }
      <Instructions exchangeName={pickedExchangeName} />
      <div className="db-bot__footer">
        <ResetButton />
      </div>
    </div>
  )
}
