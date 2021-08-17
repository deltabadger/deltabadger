import React, {useEffect, useState} from 'react'
import I18n from 'i18n-js'
import { RawHTML } from '../RawHtml'
import { Instructions } from './Instructions';
import { Breadcrumbs } from './Breadcrumbs'
import { Progressbar } from './Progressbar'
import { getExchange } from '../../lib/exchanges'
import {Spinner} from "../Spinner";

const apiKeyNames = exchangeName => {
  const { translation_key } = getExchange(exchangeName)

  return {
    private: I18n.t('bots.setup.' + translation_key + '.private_key'),
    public: I18n.t('bots.setup.' + translation_key + '.public_key'),
    passphrase: I18n.t('bots.setup.' + translation_key + '.passphrase')
  }
}

const isPassphraseRequired = exchangeName => {
  return ['Coinbase Pro', 'KuCoin'].includes(exchangeName)
}

export const AddApiKey = ({
  pickedExchangeName,
  handleReset,
  handleSubmit,
  handleRemove,
  status,
  botView
}) => {
  const [key, setKey] = useState("");
  const [secret, setSecret] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [agreement, setAgreement] = useState(false);
  const uniqueId = new Date().getTime();

  const ResetButton = () => (
    <div
      onClick={() => handleReset()}
      className="btn btn-link btn--reset btn--reset-back"
    >
      <i className="material-icons-round">close</i>
      <span>{I18n.t('bots.setup.cancel')}</span>
    </div>
  )

  const disableSubmit = key == '' || secret == '' || (isPassphraseRequired(pickedExchangeName)  && passphrase == '')

  const disableFormFields = status == 'validating_api_key'

  const _handleSubmit = (evt) => {
      evt.preventDefault();
      !disableSubmit && handleSubmit(key, secret, passphrase, agreement)
  }

  const { public: key_label, private: secret_label, passphrase: phrase_label } = apiKeyNames(pickedExchangeName);

  useEffect(() => {
    if (status === 'invalid_api_key') {
      document.getElementById(`api-key${uniqueId}`).setCustomValidity("Error")
      document.getElementById(`api-secret${uniqueId}`).setCustomValidity("Error")
      if (isPassphraseRequired(pickedExchangeName)) {
        document.getElementById(`api-passphrase${uniqueId}`).setCustomValidity("Error")
      }

      handleRemove()
    }
  })

  return (
    <div className="db-bots__item db-bot db-bot--get-apikey db-bot--active">
      <div className="db-bot__header">
        { !botView && <Breadcrumbs step={1} /> }
        { (status == 'add_api_key' || status == 'invalid_api_key') &&
          <div onClick={_handleSubmit} className={`btn ${disableSubmit ? 'btn-outline-secondary disabled' : 'btn-outline-primary'}`}>
            <span>{botView ? I18n.t('bots.setup.set') : I18n.t('bots.setup.next')}</span>
            <svg className="db-bot__svg-icon db-svg-icon db-svg-icon--arrow-forward" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M5 13h11.2l-5 4.9a1 1 0 000 1.4c.5.4 1.1.4 1.5 0l6.6-6.6c.4-.4.4-1 0-1.4l-6.6-6.6a1 1 0 10-1.4 1.4l4.9 4.9H5c-.6 0-1 .5-1 1s.5 1 1 1z"/></svg>
          </div>
        }
        { status == 'validating_api_key' &&
          <div>
            <div className="db-bot__infotext__right">{I18n.t('bots.setup.validating')}</div>
            <Spinner />
          </div>
        }
        <div className="db-bot__infotext">
          { botView &&
            <div className="db-bot__infotext__left">
              { pickedExchangeName }:{botView.baseName}{botView.quoteName}
            </div>
          }
        </div>
      </div>
      <Progressbar value={33}/>
      <div className="db-bot__form db-bot__form--apikeys">
        <form onSubmit={_handleSubmit} className="form-row">
          <div className="col">
            <div className="db-form__row mb-0">
              <input
                id={`api-key${uniqueId}`}
                type="text"
                value={key}
                onChange={e => setKey(e.target.value)}
                className="db-form__input"
                disabled={disableFormFields}
                autoComplete="off"
              />
              <div className="db-form__info db-form__info--invalid">
                {I18n.t('bots.setup.error_info')}
              </div>
              <label htmlFor="api-key" className="db-form__label">{ key_label }</label>
            </div>
          </div>
          <div className="col">
            <div className="db-form__row mb-0">
              <input
                id={`api-secret${uniqueId}`}
                type="text"
                value={secret}
                onChange={e => setSecret(e.target.value)}
                className="db-form__input"
                disabled={disableFormFields}
                autoComplete="off"
              />
              <div className="db-form__info db-form__info--invalid">
              </div>
              <label htmlFor="api-secret" className="db-form__label">{ secret_label }</label>
            </div>
          </div>
          { isPassphraseRequired(pickedExchangeName) &&
            <div className="col">
              <div className="db-form__row mb-0">
                <input
                  id={`api-passphrase${uniqueId}`}
                  type="text"
                  value={passphrase}
                  onChange={e => setPassphrase(e.target.value)}
                  className="db-form__input"
                  disabled={disableFormFields}
                />
                <label htmlFor="api-passphrase" className="db-form__label">{ phrase_label }</label>
              </div>
            </div>
          }
        </form>
      </div>
      { pickedExchangeName == "Kraken" &&
        <div className="alert alert-primary alert-trading-agreement">
          <div className="alert__regular-text">
            <RawHTML tag="p">{I18n.t('bots.setup.kraken.trading_agreement_html')}</RawHTML>
            <div className="form-check">
              <input
                id="trading-agreement"
                type="checkbox"
                checked={agreement}
                onChange={_ => setAgreement(!agreement)}
                className="form-check-input"
                disabled={disableFormFields}
              />
              <label htmlFor="trading-agreement" className="form-check-label">
                <RawHTML tag="b">{I18n.t('bots.setup.kraken.trading_agreement_label_html')}</RawHTML>
              </label>
            </div>
          </div>
        </div>
      }
      <Instructions exchangeName={pickedExchangeName} />
      { !botView &&
        <div className="db-bot__footer">
          <ResetButton/>
        </div>
      }
    </div>
  )
}
