import React, { useState } from 'react'
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'

const userHasApiToken = () => {
}

const STEPS = [
  'closed_form',
  'pick_exchange',
  'add_api_key' ,
  'configure_bot',
]

const getExchanges = () => ([
  { id: 1, name: "Kraken" },
  { id: 2, name: "Bitcos" },
  { id: 3, name: "Luxmed" },
])

const ClosedForm = ({ handleSubmit }) => (
  <div>
    <button onClick={handleSubmit}>
      Add bot
    </button>
  </div>
)

const PickExchage = ({ handleSubmit }) => {
  const exchanges = getExchanges()
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

const AddApiKey = ({ handleSubmit }) => {
  return(<h1>set api key</h1>)
}

export const BotForm = props => {
  const [step, setStep] = useState(0);
  const [form, setFormState] = useState({exchangeId: null, api_key: null, bot_params: {}});

  const pickExchangeHandler = (id) => {
    setFormState({...form, exchangeId: id})
    setStep(step + 1)
  }

  const addApiKeyHandler = () => {
    setStep(step + 1)
  }

  if (step == STEPS[step] && userHasApiToken()) { step += 1 }
  switch (STEPS[step]) {
    case 'closed_form':
      return <ClosedForm handleSubmit={() => setStep(step + 1)} />
    case 'pick_exchange':
      return <PickExchage handleSubmit={pickExchangeHandler}  />
    case 'add_api_key':
      return <AddApiKey handleSubmit={addApiKeyHandler}  />
    case 'configure_bot':
      return <PickExchage handleSubmit={addApiKeyHandler}  />
  }
}
