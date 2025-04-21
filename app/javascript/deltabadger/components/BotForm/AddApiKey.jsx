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
    private: I18n.t('bot.setup.' + translation_key + '.private_key'),
    public: I18n.t('bot.setup.' + translation_key + '.public_key'),
    passphrase: I18n.t('bot.setup.' + translation_key + '.passphrase')
  }
}

const isPassphraseRequired = exchangeName => {
  return ['Coinbase Pro', 'KuCoin'].includes(exchangeName)
}
const NOT_RELEVANT_BOTS = ["FTX", "FTX.US", "Coinbase Pro"];

export const AddApiKey = ({
  pickedExchangeName,
  handleReset,
  handleSubmit,
  handleRemove,
  status,
  botView,
  type
}) => {
  const [key, setKey] = useState("");
  const [secret, setSecret] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [agreement, setAgreement] = useState(false);
  const [showError, setShowError] = useState(false)
  const uniqueId = new Date().getTime();
  const ResetButton = () => (
    <div
      onClick={() => handleReset()}
      className="button button--link"
    >
      <i className="material-icons">close</i>
      <span>{I18n.t('button.cancel')}</span>
    </div>
  )

  const disableSubmit = key == '' || secret == '' || (isPassphraseRequired(pickedExchangeName)  && passphrase == '')

  const disableFormFields = status == 'validating_api_key'

  const _handleSubmit = (evt) => {
      evt.preventDefault();

      if(!disableSubmit) {
        if(!NOT_RELEVANT_BOTS.includes(pickedExchangeName)) {
          setShowError(false);

          handleSubmit(key, secret, passphrase, agreement, type);
        } else {
          setShowError(true);
        }
      }
  }

  const getBotName = () => {
    if(!botView)
      return '';

    return botView.baseName ? `${botView.baseName}${botView.quoteName}` : `${botView.currencyName}`
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
    <div>
      <div className="db-bot__header">
        { !botView && <Breadcrumbs step={2} /> }
        { (status == 'add_api_key' || status == 'invalid_api_key') &&
          <div onClick={_handleSubmit} className={`button ${disableSubmit ? 'button--outline button--disabled' : 'button--primary'}`}>
            <span>{botView ? I18n.t('bot.setup.set') : I18n.t('bot.setup.next')}</span>
          </div>
        }
        { status == 'validating_api_key' &&
          <div>
            <div className="db-bot__infotext__right">{I18n.t('bot.setup.validating')}</div>
            <Spinner />
          </div>
        }
        <div className="db-bot__infotext">
          { botView &&
            <div className="db-bot__infotext__left">
              { pickedExchangeName }:{getBotName()}
            </div>
          }
        </div>
      </div>
      <Progressbar value={33}/>
        <form onSubmit={_handleSubmit} className="old-bot-api-key-form">
          <div className="db-form__row">
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
              {I18n.t('bot.setup.error_info')}
            </div>
            {showError && <div className="db-form__info db-form__info--invalid" style={{display: 'block'}}>{I18n.t('bot.setup.api_not_available')}</div>}
            <label htmlFor="api-key" className="db-form__label">{ key_label }</label>
          </div>
          <div className="db-form__row">
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
          { isPassphraseRequired(pickedExchangeName) &&
            <div className="db-form__row">
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
          }
        </form>
      { pickedExchangeName == "Kraken" &&
        <div className="alert alert-primary alert-trading-agreement">
          <div className="alert__regular-text">
            <RawHTML tag="p">{I18n.t('bot.setup.kraken.trading_agreement_html')}</RawHTML>
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
                <RawHTML tag="b">{I18n.t('bot.setup.kraken.trading_agreement_label_html')}</RawHTML>
              </label>
            </div>
          </div>
        </div>
      }
      <Instructions exchangeName={pickedExchangeName} type={type} />
      { !botView &&
        <div className="bot-footer">
          <ResetButton/>
        </div>
      }
    </div>
  )
}
