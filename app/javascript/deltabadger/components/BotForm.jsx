import React, { useState, useEffect } from 'react'
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'
import API from '../lib/API'

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

const ConfigureBot = ({ handleSubmit }) => {
  const [type, setType] = useState("sell");
  const [price, setPrice] = useState("");
  const [currency, setCurrency] = useState("USD");
  const [interval, setInterval] = useState("month");

  const _handleSubmit = (evt) => {
      evt.preventDefault();
      const botParams = { type, price, currency, interval}
      handleSubmit(botParams);
  }

  return (
      <form onSubmit={_handleSubmit}>
        <select value={type} onChange={e => setType(e.target.value)}>
          <option value="sell">Sell</option>
          <option value="buy">Buy</option>
        </select>

        for
        <input
          type="text"
          value={price}
          onChange={e => setPrice(e.target.value)}
        />

        <select value={currency} onChange={e => setCurrency(e.target.value)}>
          <option value="USD">USD</option>
          <option value="EUR">EUR</option>
        </select>
        /
        <select value={interval} onChange={e => setInterval(e.target.value)}>
          <option value="month">month</option>
          <option value="week">week</option>
        </select>


        <input type="submit" value="Submit" />
      </form>
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
