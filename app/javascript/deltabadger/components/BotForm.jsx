import React, { useState, useEffect } from 'react'
import API from '../lib/API'
import I18n from 'i18n-js'
import { PickExchage } from './BotForm/PickExchange';
import { ConfigureBot } from './BotForm/ConfigureBot';
import { AddApiKey } from './BotForm/AddApiKey';
import { Details } from './BotForm/Details';
import { removeInvalidApiKeys } from "./helpers";
import { NavigationPanel } from "./BotForm/NavigationPanel";

const STEPS = [
  'closed_form',
  'pick_exchange',
  'add_api_key' ,
  'validating_api_key',
  'invalid_api_key',
  'configure_bot',
]

const TYPES = [
  'trading',
  'withdrawal'
]

export const BotForm = ({
  isHodler,
  open,
  currentBot,
  callbackAfterCreation,
  callbackAfterOpening,
  callbackAfterClosing,
  exchanges,
  fetchExchanges,
  apiKeyTimeout,
  page,
  setPage,
  numberOfPages,
  step,
  setStep
}) => {
  const [form, setFormState] = useState({});
  const [errors, setErrors] = useState("");
  const [type, setType] = useState(TYPES[0])
  const [isCreatingBot, setCreatingBot] = useState(false);

  const getKeyStatus = (e) => {
    return type === 'trading' ? e.trading_key_status : e.withdrawal_key_status
  }

  const pickedExchange = exchanges.find(e => form.exchangeId == e.id) || {}
  const ownedExchangesIds = exchanges.filter(e => getKeyStatus(e) === 'correct').map(e => e.id)
  const pendingExchangesIds = exchanges.filter(e => getKeyStatus(e) === 'pending').map(e => e.id)
  let invalidExchangesIds = exchanges.filter(e => getKeyStatus(e) === 'incorrect').map(e => e.id)
  console.log(exchanges, ownedExchangesIds)

  const keyExists = (exchangeId) => {
    return [...ownedExchangesIds, ...invalidExchangesIds, ...pendingExchangesIds].includes(exchangeId)
  }

  const clearAndSetTimeout = () => {
    clearTimeout(apiKeyTimeout)
    apiKeyTimeout = setTimeout(() => fetchExchanges(type), 3000)
  }

  const chooseStep = step => {
    if ((STEPS[step] == 'add_api_key') && ownedExchangesIds.includes(form.exchangeId)) { return 5 }
    if ((STEPS[step] == 'add_api_key') && invalidExchangesIds.includes(form.exchangeId)) { return 4 }
    if ((STEPS[step] == 'add_api_key') && pendingExchangesIds.includes(form.exchangeId)) {
      clearAndSetTimeout()
      return 3
    }

    if ((STEPS[step] == 'validating_api_key') && ownedExchangesIds.includes(form.exchangeId)) { return 5 }
    if ((STEPS[step] == 'validating_api_key') && invalidExchangesIds.includes(form.exchangeId)) { return 4 }

    if ((STEPS[step] == 'closed_form') && open) { return step + 1 }

    if ((STEPS[step] == 'validating_api_key') && !keyExists(form.exchangeId)) {
      return 4
    }

    if((STEPS[step] == 'validating_api_key')) {
      clearAndSetTimeout()
    }

    return step;
  }

  useEffect(() => {
    if (currentBot) {
      setStep(0)
      setErrors([])
      setFormState({})
    }
  }, [currentBot])

  const closedFormHandler = (type) => {
    setPage(1)
    setStep(1)
    setType(type)
    callbackAfterOpening()
  }

  const pickExchangeHandler = (id, name) => {
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

  const addApiKeyHandler = (key, secret, passphrase, germanAgreement, type) => {
    setPendingStatus()
    API.createApiKey({ key, secret, passphrase, germanAgreement, type, exchangeId: form.exchangeId }).then(response => {
      setErrors([])
      setStep(3)
    }).catch(() => {
      setErrors(I18n.t('errors.invalid_api_keys'))
    })
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

  const handleCancel = () => {
    setStep(0)
    callbackAfterClosing()
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
        clearTimeout(apiKeyTimeout)
        return <AddApiKey
          pickedExchangeName={pickedExchange.name}
          handleReset={resetFormToStep(1)}
          handleSubmit={addApiKeyHandler}
          handleRemove={() => removeInvalidApiKeys(form.exchangeId)}
          status={'add_api_key'}
          type={type}
        />
      case 'validating_api_key':
        return <AddApiKey
          pickedExchangeName={pickedExchange.name}
          handleReset={resetFormToStep(1)}
          handleSubmit={addApiKeyHandler}
          handleRemove={() => removeInvalidApiKeys(form.exchangeId)}
          status={'validating_api_key'}
          type={type}
        />
      case 'invalid_api_key':
        clearTimeout(apiKeyTimeout)
        return <AddApiKey
          pickedExchangeName={pickedExchange.name}
          handleReset={resetFormToStep(1)}
          handleSubmit={addApiKeyHandler}
          handleRemove={() => removeInvalidApiKeys(form.exchangeId)}
          status={'invalid_api_key'}
          type={type}
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
    <NavigationPanel
      handleCancel={handleCancel}
      closedFormHandler={closedFormHandler}
      step={chooseStep(step)}
      page={page}
      setPage={setPage}
      numberOfPages={numberOfPages}
    />
    { renderForm() }
    { chooseStep(step) > 0 && <Details /> }
    </>
  )
}
