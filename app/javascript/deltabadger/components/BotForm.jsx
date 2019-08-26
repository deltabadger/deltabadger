import React, { useState, useEffect } from 'react'
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'
import API from '../lib/API'

const userHasApiToken = () => {
}

const STEPS = [
  'closed_form',
  'pick_exchange',
  'add_api_key' ,
  'configure_bot',
]

const ClosedForm = ({ handleSubmit }) => (
  <div>
    <button onClick={handleSubmit}>
      Add bot
    </button>
  </div>
)

const PickExchage = ({ handleSubmit, exchanges }) => {
  const ExchangeButton = ({ handleClick, exchange }) => (
    <button onClick={ () => handleClick(exchange.id) }>
      { exchange.name }
    </button>
  )

  return (
    <div>
      {
        exchanges.map(e =>
          <ExchangeButton key={e.id} handleClick={handleSubmit} exchange={e} />
        )
      }
    </div>
  )

}

const AddApiKey = ({ handleSubmit, errors }) => {
  const [key, setKey] = useState("");

  const _handleSubmit = (evt) => {
      evt.preventDefault();
      handleSubmit(key)
  }

  return (
    <div>
      { errors }
      <form onSubmit={_handleSubmit}>
        <label>
          Api-Key:
          <input
            type="text"
            value={key}
            onChange={e => setKey(e.target.value)}
          />
        </label>
        <input type="submit" value="Submit" />
      </form>
    </div>
  )
}

const ConfigureBot = ({ handleSubmit }) => {
  return (
    <h1>Configure bot</h1>
  )
}


export const BotForm = props => {
  const [step, setStep] = useState(0);
  const [form, setFormState] = useState({exchangeId: null, api_key: null, bot_params: {}});
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
  });

  const pickExchangeHandler = (id) => {
    setFormState({...form, exchangeId: id})
    setStep(step + 1)
  }

  const addApiKeyHandler = (key) => {
    API.createApiKey({ key, exchangeId: form.exchangeId }).then(response => {
      setErrors([])
      setStep(step + 1)
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
      return <ConfigureBot handleSubmit={addApiKeyHandler}  />
  }
}
