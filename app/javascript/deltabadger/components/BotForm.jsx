import React, { useState, useEffect } from 'react'
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'
import API from '../lib/API'
import { PickExchage } from './BotForm/PickExchange';
import { ConfigureBot } from './BotForm/ConfigureBot';
import { AddApiKey } from './BotForm/AddApiKey';
import { ClosedForm } from './BotForm/ClosedForm';
import { Details } from './BotForm/Details';

const STEPS = [
  'closed_form',
  'pick_exchange',
  'add_api_key' ,
  'configure_bot',
]

export const BotForm = ({
  open,
  currentBot,
  callbackAfterCreation,
  callbackAfterOpening,
  callbackAfterClosing
}) => {
  const [step, setStep] = useState(0);
  const [form, setFormState] = useState({});
  const [exchanges, setExchanges] = useState([]);
  const [errors, setErrors] = useState("");

  const pickedExchange = exchanges.find(e => form.exchangeId == e.id) || {}
  const ownedExchangesIds = exchanges.filter(e => e.owned).map(e => e.id)

  const chooseStep = step => {
    if ((STEPS[step] == 'add_api_key') && ownedExchangesIds.includes(form.exchangeId)) { return step + 1 }
    if ((STEPS[step] == 'closed_form') && open) { return step + 1 }

    return step;
  }

  const loadExchanges = () => {
    API.getExchanges().then(data => {
      setExchanges(data.data)
    })
  }

  useEffect(() => {
    loadExchanges()
  }, []);

  useEffect(() => {
    if (currentBot) {
      setStep(0)
      setErrors([])
      setFormState({})
    }
  }, [currentBot])

  const closedFormHandler = () => {
    setStep(1)
    callbackAfterOpening()
  }

  const pickExchangeHandler = (id) => {
    setFormState({...form, exchangeId: id})
    setStep(2)
  }

  const addApiKeyHandler = (key, secret) => {
    API.createApiKey({ key, secret, exchangeId: form.exchangeId }).then(response => {
      setErrors([])
      setStep(3)
      loadExchanges()
    }).catch(() => {
      setErrors("Invalid token")
    })
  }

  const configureBotHandler = (botParams) => {
    const params = {...botParams, exchangeId: form.exchangeId}
    API.createBot(params).then(response => {
      callbackAfterCreation(response.data.id)
      setErrors([])
      setStep(0)
      setFormState({})
    }).catch(() => {
      setErrors("Invalid token")
    })
  }

  // TODO: Fix this!, you can't reset all form, check this!
  const resetFormToStep = (step) => {
    return(() => {
      setErrors([])
      setFormState({})
      setStep(step)
    })
  }

	const renderForm = () => {
		switch (STEPS[chooseStep(step)]) {
			case 'closed_form':
				return <ClosedForm
					handleSubmit={closedFormHandler}
				/>
			case 'pick_exchange':
				return <PickExchage
          handleReset={() => {
            setStep(0)
            callbackAfterClosing()
          }}
					handleSubmit={pickExchangeHandler}
					exchanges={exchanges}
				/>
			case 'add_api_key':
				return <AddApiKey
					pickedExchangeName={pickedExchange.name}
					handleReset={resetFormToStep(1)}
					handleSubmit={addApiKeyHandler}
					errors={errors}
				/>
			case 'configure_bot':
				return <ConfigureBot
					handleReset={resetFormToStep(1)}
					handleSubmit={configureBotHandler}
				/>
		}
	}

  return (
    <>
      { renderForm() }
      { step > 0 && <Details /> }
    </>
    )
}
