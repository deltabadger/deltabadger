import React, { useState } from 'react'
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'

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

const ClosedForm = props => (
  <div>
    <button onClick={props.handleSubmit}>
      Add bot
    </button>
  </div>
)

const PickExchage = props => {
  const exchanges = getExchanges()
  const ExchangeButton = props => (
    <button onClick={ () => props.handleClick(props.exchange.id) }>
      { props.exchange.name }
    </button>
  )

  return (
    <div>
      { exchanges.map(e => <ExchangeButton key={e.id} handleClick={props.handleSubmit} exchange={e} />) }
    </div>
  )

}

const AddApiKey = props => {
  return(<h1>set api key</h1>)
}

const BotForm = props => {
  const [step, setStep] = useState(0);
  const [form, setFormState] = useState({exchangeId: null, api_key: null, bot_params: {}});

  const pickExchangeHandler = (id) => {
    setFormState({...form, exchangeId: id})
    setStep(step + 1)
  }

  const addApiKeyHandler = () => {
    setStep(step + 1)
  }

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

document.addEventListener('DOMContentLoaded', () => {
  ReactDOM.render(
    <BotForm />,
    document.getElementById('bot_form')
  )
})
