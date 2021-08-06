import React, { useState, useEffect } from 'react'
import API from '../lib/API'
import I18n from 'i18n-js'
import { PickExchage } from './BotForm/PickExchange';
import { ConfigureBot } from './BotForm/ConfigureBot';
import { AddApiKey } from './BotForm/AddApiKey';
import { ClosedForm } from './BotForm/ClosedForm';
import { Details } from './BotForm/Details';

const STEPS = [
  'closed_form',
  'pick_exchange',
  'add_api_key' ,
  'validating_api_key',
  'invalid_api_key',
  'configure_bot',
]

export const BotForm = ({
  isHodler,
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
  const [isCreatingBot, setCreatingBot] = useState(false);

  const pickedExchange = exchanges.find(e => form.exchangeId == e.id) || {}
  const ownedExchangesIds = exchanges.filter(e => e.owned).map(e => e.id)
  const pendingExchangesIds = exchanges.filter(e => e.pending).map(e => e.id)
  let invalidExchangesIds = exchanges.filter(e => e.invalid).map(e => e.id)
  const shouldDisableHodlerOnlyExchange = (name) => {
    return !isHodler && ['ftx','ftx.us'].includes(name.toLowerCase())
  }

  const keyExists = (exchangeId) => {
    return [...ownedExchangesIds, ...invalidExchangesIds, ...pendingExchangesIds].includes(exchangeId)
  }

  const chooseStep = step => {
    if ((STEPS[step] == 'add_api_key') && ownedExchangesIds.includes(form.exchangeId)) { return 5 }
    if ((STEPS[step] == 'add_api_key') && invalidExchangesIds.includes(form.exchangeId)) { return 4 }
    if ((STEPS[step] == 'add_api_key') && pendingExchangesIds.includes(form.exchangeId)) {
      setTimeout(() => loadExchanges(), 3000)
      return 3
    }

    if ((STEPS[step] == 'validating_api_key') && ownedExchangesIds.includes(form.exchangeId)) { return 5 }
    if ((STEPS[step] == 'validating_api_key') && invalidExchangesIds.includes(form.exchangeId)) { return 4 }

    if ((STEPS[step] == 'closed_form') && open) { return step + 1 }

    if ((STEPS[step] == 'validating_api_key') && !keyExists(form.exchangeId)) {
      return 4
    }

    if((STEPS[step] == 'validating_api_key')) {
      setTimeout(() => loadExchanges(), 3000)
    }

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

  const pickExchangeHandler = (id, name) => {
    if (shouldDisableHodlerOnlyExchange(name)){
      return;
    }

    setFormState({...form, exchangeId: id})
    setStep(2)
  }

  const setPendingStatus = () => {
    const idx = exchanges.findIndex(e => e.id === form.exchangeId)
    if (idx === -1){
      return
    }

    exchanges[idx].invalid = false
    exchanges[idx].pending = true
  }

  const addApiKeyHandler = (key, secret, passphrase, germanAgreement) => {
    setPendingStatus()
    API.createApiKey({ key, secret, passphrase, germanAgreement, exchangeId: form.exchangeId }).then(response => {
      setErrors([])
      setStep(3)
    }).catch(() => {
      setErrors(I18n.t('errors.invalid_api_keys'))
    })
  }

  const removeInvalidApiKeys = () => {
    API.removeInvalidApiKeys({ exchangeId: form.exchangeId })
  }

  const getOfferTypeParams = (type) => {
    const [order_type, offer_type] = type.split('_')
    return {
      type: offer_type,
      order_type
    }
  }

  const getSmartIntervalsInfo = (botParams) => {
    const typeParams = getOfferTypeParams(botParams.type)
    const params = {...botParams, ...typeParams, exchangeId: form.exchangeId}

    return API.getSmartIntervalsInfo(params).then((data) => {
      return data
    }).catch((data) => {
      return {data: { showSmartIntervalsInfo: false }}
    })
  }

  const setShowSmartIntervalsInfo = () => {
    API.setShowSmartIntervalsInfo().then(data => data)
  }

  const configureBotHandler = (botParams) => {
    const typeParams = getOfferTypeParams(botParams.type)
    const params = {...botParams, ...typeParams, exchangeId: form.exchangeId}
    setCreatingBot(true);
    API.createBot(params).then(response => {
      callbackAfterCreation(response.data.id)
      setErrors([])
      setStep(0)
      setFormState({})
    }).catch((data) => {
      setErrors(data.response.data.errors[0])
    }).finally(() => setCreatingBot(false));
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
          shouldDisableExchange={shouldDisableHodlerOnlyExchange}
        />
      case 'add_api_key':
        return <AddApiKey
          pickedExchangeName={pickedExchange.name}
          handleReset={resetFormToStep(1)}
          handleSubmit={addApiKeyHandler}
          handleRemove={removeInvalidApiKeys}
          status={'add_api_key'}
        />
      case 'validating_api_key':
        return <AddApiKey
          pickedExchangeName={pickedExchange.name}
          handleReset={resetFormToStep(1)}
          handleSubmit={addApiKeyHandler}
          handleRemove={removeInvalidApiKeys}
          status={'validating_api_key'}
        />
      case 'invalid_api_key':
        return <AddApiKey
          pickedExchangeName={pickedExchange.name}
          handleReset={resetFormToStep(1)}
          handleSubmit={addApiKeyHandler}
          handleRemove={removeInvalidApiKeys}
          status={'invalid_api_key'}
        />
      case 'configure_bot':
        return <ConfigureBot
          showLimitOrders={isHodler}
          currentExchange={pickedExchange}
          handleReset={resetFormToStep(1)}
          handleSubmit={configureBotHandler}
          handleSmartIntervalsInfo={getSmartIntervalsInfo}
          setShowInfo={setShowSmartIntervalsInfo}
          disable={isCreatingBot}
          errors={errors}
        />
    }
  }

  return (
    <>
    { renderForm() }
    { chooseStep(step) > 0 && <Details /> }
    </>
  )
}
