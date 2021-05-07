import React, { useState } from 'react'
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

export const AddApiKey = ({
  pickedExchangeName,
  handleReset,
  handleSubmit,
  handleTryAgain,
  handleRemove,
  status
}) => {
  const [key, setKey] = useState("");
  const [secret, setSecret] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [agreement, setAgreement] = useState(false)

  const ResetButton = () => (
    <div
      onClick={() => handleReset()}
      className="btn btn-link btn--reset btn--reset-back"
    >
      <i className="material-icons-round">close</i>
      <span>{I18n.t('bots.setup.cancel')}</span>
    </div>
  )

  const disableSubmit = key == '' || secret == '' || (pickedExchangeName == 'Coinbase Pro' && passphrase == '')

  const disableFormFields = status == 'validating_api_key'

  const _handleSubmit = (evt) => {
      evt.preventDefault();
      !disableSubmit && handleSubmit(key, secret, passphrase, agreement)
  }

  const _handleRemove = (evt) => {
    evt.preventDefault();
    !disableSubmit && handleRemove()
    !disableSubmit && handleSubmit(key, secret, passphrase, agreement)
  }

  const { public: key_label, private: secret_label, passphrase: phrase_label } = apiKeyNames(pickedExchangeName);

  return (
    <div className="db-bots__item db-bot db-bot--get-apikey db-bot--active">
      <div className="db-bot__header">
        <Breadcrumbs step={1} />
        { status == 'add_api_key' &&
          <div onClick={_handleSubmit} className={`btn ${disableSubmit ? 'btn-outline-secondary disabled' : 'btn-outline-primary'}`}>
            <span>{I18n.t('bots.setup.next')}</span>
            <svg className="db-bot__svg-icon db-svg-icon db-svg-icon--arrow-forward" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M5 13h11.2l-5 4.9a1 1 0 000 1.4c.5.4 1.1.4 1.5 0l6.6-6.6c.4-.4.4-1 0-1.4l-6.6-6.6a1 1 0 10-1.4 1.4l4.9 4.9H5c-.6 0-1 .5-1 1s.5 1 1 1z"/></svg>
          </div>
        }
        { status == 'validating_api_key' &&
          <div>
            <div className="db-bot__infotext__right">Validating</div>
            <Spinner />
          </div>
        }
        <div className="db-bot__infotext">
        </div>
      </div>
      <Progressbar value={33}/>
      <div className="db-bot__form db-bot__form--apikeys">
        {status == 'invalid_api_key' &&
          <div className="db-bot__alert text-danger">
            Wrong keys or insufficient permissions. You can check your permissions and try to validate the same keys again
            or you can add new API keys by providing them below.
          </div>
        }
        <form onSubmit={_handleSubmit} className="form-row">
          <div className="col">
            <div className="db-form__row mb-0">
              <input
                id="api-key"
                type="text"
                value={key}
                onChange={e => setKey(e.target.value)}
                className="db-form__input"
                disabled={disableFormFields}
              />
              <label htmlFor="api-key" className="db-form__label">{ key_label }</label>
            </div>
          </div>
          <div className="col">
            <div className="db-form__row mb-0">
              <input
                id="api-secret"
                type="text"
                value={secret}
                onChange={e => setSecret(e.target.value)}
                className="db-form__input"
                disabled={disableFormFields}
              />
              <label htmlFor="api-secret" className="db-form__label">{ secret_label }</label>
            </div>
          </div>
          { pickedExchangeName == "Coinbase Pro" &&
            <div className="col">
              <div className="db-form__row mb-0">
                <input
                  id="api-passphrase"
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
        { status == 'invalid_api_key' &&
          <div className="db-bot__form db-bot__form--apikeys">
            <div>
              <div onClick={() => handleTryAgain()} className="btn btn-outline-primary">
                Try again
              </div>
              <div onClick={_handleRemove} className={`btn btn-success ${disableSubmit ? 'disabled' : ''}`}>
                Add new API keys
              </div>
            </div>
          </div>
        }
      </div>
      { pickedExchangeName == "Kraken" &&
        <div className="db-exchange-instructions">
          <div className="alert alert--trading-agreement">
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
      <div className="db-bot__footer">
        <ResetButton />
      </div>
    </div>
  )
}
