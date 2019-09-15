import React, { useState, useEffect } from 'react'
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'
import API from '../lib/API'
import { PickExchage } from './BotForm/PickExchange';
import { ConfigureBot } from './BotForm/ConfigureBot';

const STEPS = [
  'closed_form',
  'pick_exchange',
  'add_api_key' ,
  'configure_bot',
]

const ClosedForm = ({ handleSubmit }) => (
  <div className="db-bots__item d-flex justify-content-center db-add-more-bots">
    <button onClick={handleSubmit} className="btn btn-link">
      Add new bot +
    </button>
  </div>
)

const AddApiKey = ({ handleSubmit, errors }) => {
  const [key, setKey] = useState("");
  const [secret, setSecret] = useState("");

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
    </div>
  )
}



const initialForm = {
  exchangeId: null,
  api_key: null,
}

export const BotForm = ({ callbackAfterCreation }) => {
  const [step, setStep] = useState(0);
  const [form, setFormState] = useState(initialForm);
  const [exchanges, setExchanges] = useState([]);
  const [errors, setErrors] = useState("");

  const ownedExchangesIds = exchanges.filter(e => e.owned).map(e => e.id)

  const chooseStep = step => {
    if ((STEPS[step] == 'add_api_key') && ownedExchangesIds.includes(form.exchangeId)) { return step + 1 }

    return step;
  }

  useEffect(() => {
    exchanges.length == 0 && API.getExchanges().then(data => {
      setExchanges(data.data)
    })
  }, []);

  const pickExchangeHandler = (id) => {
    setFormState({...form, exchangeId: id})
    setStep(step + 1)
  }

  const addApiKeyHandler = (key, secret) => {
    API.createApiKey({ key, secret, exchangeId: form.exchangeId }).then(response => {
      setErrors([])
      setStep(step + 1)
    }).catch(() => {
      setErrors("Invalid token")
    })
  }

  const configureBotHandler = (botParams) => {
    const params = {...botParams, exchangeId: form.exchangeId}
    API.createBot(params).then(response => {
      callbackAfterCreation()
      console.log(params)
      setErrors([])
      setStep(0)
      setForm(initialForm)
    }).catch(() => {
      setErrors("Invalid token")
    })
  }

  switch (STEPS[chooseStep(step)]) {
    case 'closed_form':
      return <ClosedForm handleSubmit={() => setStep(step + 1)} />
    case 'pick_exchange':
      return <PickExchage handleSubmit={pickExchangeHandler} exchanges={exchanges} />
    case 'add_api_key':
      return <AddApiKey handleSubmit={addApiKeyHandler} errors={errors}  />
    case 'configure_bot':
      return <ConfigureBot handleSubmit={configureBotHandler}  />
  }
}
