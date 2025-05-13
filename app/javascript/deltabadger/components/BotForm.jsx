import React, { useState, useEffect } from 'react'
import API from '../lib/API'
import I18n from 'i18n-js'
import { PickExchange } from './BotForm/PickExchange';
import { ConfigureTradingBot } from './BotForm/ConfigureTradingBot';
import { AddApiKey } from './BotForm/AddApiKey';
import { Details } from './BotForm/Details';
import { removeInvalidApiKeys } from "./helpers";
import { ConfigureWithdrawalBot } from "./BotForm/ConfigureWithdrawalBot";
import { ConfigureWebhookBot } from './BotForm/ConfigureWebhookBot';
import { PickBotType } from "./BotForm/PickBotType";

const STEPS = [
  'closed_form',
  'pick_bot_type',
  'pick_exchange',
  'add_api_key' ,
  'validating_api_key',
  'invalid_api_key',
  'configure_trading_bot',
  'configure_withdrawal_bot',
  'configure_webhook_bot'
]

const TYPES = [
  'trading',
  'withdrawal',
  'webhook'
]

export const BotForm = ({
  isBasic,
  isPro,
  isLegendary,
  open,
  currentBot,
  callbackAfterCreation,
  callbackAfterOpening,
  callbackAfterClosing,
  exchanges,
  fetchExchanges,
  apiKeyTimeout,
  step,
  setStep
}) => {
  const [form, setFormState] = useState({});
  const [errors, setErrors] = useState("");
  const [type, setType] = useState(TYPES[0])
  const [isCreatingBot, setCreatingBot] = useState(false);

  const getKeyStatus = (e) => {
    // console.log("getKeyStatus");
    // console.log(step);
    // console.log(e.trading_key_status );
    // console.log(e.withdrawal_key_status);
    return type === 'withdrawal' ? e.withdrawal_key_status : e.trading_key_status
    // return type === 'trading' ? e.trading_key_status : e.withdrawal_key_status
  }

  const pickedExchange = exchanges.find(e => form.exchangeId == e.id) || {}
  const ownedExchangesIds = exchanges.filter(e => getKeyStatus(e) === 'correct').map(e => e.id)
  const pendingExchangesIds = exchanges.filter(e => getKeyStatus(e) === 'pending').map(e => e.id)
  let invalidExchangesIds = exchanges.filter(e => getKeyStatus(e) === 'incorrect').map(e => e.id)

  const keyExists = (exchangeId) => {
    return [...ownedExchangesIds, ...invalidExchangesIds, ...pendingExchangesIds].includes(exchangeId)
  }

  const clearAndSetTimeout = () => {
    clearTimeout(apiKeyTimeout)
    apiKeyTimeout = setTimeout(() => fetchExchanges(type), 3000)
  }

  const chooseStep = step => {
    // console.log("start chooseStep");
    // console.log(step);
    // console.log(STEPS[step]);
    // console.log(type);

    // if ((STEPS[step] == 'add_api_key') && ownedExchangesIds.includes(form.exchangeId)) { return type === 'trading' ? 6 : 7 }
    if ((STEPS[step] == 'add_api_key') && ownedExchangesIds.includes(form.exchangeId)) {
      // debugger
      switch (type) {
        case 'trading':
          return 6;
        case 'withdrawal':
          return 7;
        case 'webhook':
          return 8;
      }
    }
    if ((STEPS[step] == 'add_api_key') && invalidExchangesIds.includes(form.exchangeId)) { return 5 }
    if ((STEPS[step] == 'add_api_key') && pendingExchangesIds.includes(form.exchangeId)) {
      clearAndSetTimeout()
      return 4
    }

    if ((STEPS[step] == 'validating_api_key') && ownedExchangesIds.includes(form.exchangeId)) {
      switch (type) {
        case 'trading':
          return 6;
        case 'withdrawal':
          return 7;
        case 'webhook':
          return 8;
      }
    }
    if ((STEPS[step] == 'validating_api_key') && invalidExchangesIds.includes(form.exchangeId)) { return 5 }

    if ((STEPS[step] == 'closed_form') && open) { return step + 1 }

    if ((STEPS[step] == 'validating_api_key') && !keyExists(form.exchangeId)) {
      return 5
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

  const pickBotTypeHandler = (type) => {
    setType(type)
    setStep(2)
  }

  const pickExchangeHandler = (id, name) => {
    setFormState({...form, exchangeId: id})
    setStep(3)
  }

  const setPendingStatus = () => {
    const idx = exchanges.findIndex(e => e.id === form.exchangeId)
    if (idx === -1){
      return
    }

    if (type === 'withdrawal') {
      exchanges[idx].withdrawal_key_status = 'pending'
    } else {
      exchanges[idx].trading_key_status = 'pending'
    }
  }

  const addApiKeyHandler = (key, secret, passphrase, germanAgreement, type) => {
    setPendingStatus()
    API.createApiKey({ key, secret, passphrase, germanAgreement, type, exchangeId: form.exchangeId }).then(response => {
      setErrors([])
      setStep(4)
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

  const getWithdrawalMinimums = (exchangeId, currency) => {
    return API.getWithdrawalMinimums(exchangeId, currency)
      .then(data => data.data)
      .catch(() => { return { minimum: 0.0 }})
  }

  const configureTradingBotHandler = (botParams) => {
    const typeParams = getOfferTypeParams(botParams.type)
    const params = {...botParams, ...typeParams, exchangeId: form.exchangeId}
    setCreatingBot(true);
    API.createTradingBot(params).then(response => {
      callbackAfterCreation(response.data.id)
      setErrors([])
      setStep(0)
      setFormState({})
    }).catch((data) => {
      setErrors(data.response.data.errors[0])
    }).finally(() => setCreatingBot(false));
  }

  const configureWithdrawalBotHandler = (botParams) => {
    const params = {...botParams, exchangeId: form.exchangeId}
    setCreatingBot(true);
    API.createWithdrawalBot(params).then(response => {
      callbackAfterCreation(response.data.id)
      setErrors([])
      setStep(0)
      setFormState({})
    }).catch((data) => {
      setErrors(data.response.data.errors[0])
    }).finally(() => setCreatingBot(false));
  }

  const configureWebhookBotHandler = (botParams) => {
    // debugger
    const params = {...botParams, exchangeId: form.exchangeId}
    setCreatingBot(true);
    API.createWebhookBot(params).then(response => {
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
      case 'pick_bot_type':
        return <PickBotType
          handleReset={() => {
            setStep(0)
            callbackAfterClosing()
          }}
          handleSubmit={pickBotTypeHandler}
          showWebhookButton={isPro || isLegendary}
          />
      case 'pick_exchange':
        return <PickExchange
          handleReset={() => {
            setStep(0)
            callbackAfterClosing()
          }}
          handleSubmit={pickExchangeHandler}
          exchanges={exchanges}
          type={type}
        />
      case 'add_api_key':
        clearTimeout(apiKeyTimeout)
        return <AddApiKey
          pickedExchangeName={pickedExchange.name}
          handleReset={resetFormToStep(1)}
          handleSubmit={addApiKeyHandler}
          handleRemove={() => removeInvalidApiKeys(form.exchangeId)}
          status={'add_api_key'}
          type={type === 'withdrawal' ? 'withdrawal' : 'trading'}
          errors={errors}
        />
      case 'validating_api_key':
        return <AddApiKey
          pickedExchangeName={pickedExchange.name}
          handleReset={resetFormToStep(1)}
          handleSubmit={addApiKeyHandler}
          handleRemove={() => removeInvalidApiKeys(form.exchangeId)}
          status={'validating_api_key'}
          type={type === 'withdrawal' ? 'withdrawal' : 'trading'}
          errors={errors}
        />
      case 'invalid_api_key':
        clearTimeout(apiKeyTimeout)
        return <AddApiKey
          pickedExchangeName={pickedExchange.name}
          handleReset={resetFormToStep(1)}
          handleSubmit={addApiKeyHandler}
          handleRemove={() => removeInvalidApiKeys(form.exchangeId)}
          status={'invalid_api_key'}
          type={type === 'withdrawal' ? 'withdrawal' : 'trading'}
          errors={errors}
        />
      case 'configure_trading_bot':
        return <ConfigureTradingBot
          showLimitOrders={isBasic || isPro || isLegendary}
          currentExchange={pickedExchange}
          handleReset={resetFormToStep(1)}
          handleSubmit={configureTradingBotHandler}
          handleSmartIntervalsInfo={getSmartIntervalsInfo}
          setShowInfo={setShowSmartIntervalsInfo}
          disable={isCreatingBot}
          errors={errors}
        />
      case 'configure_withdrawal_bot':
        return <ConfigureWithdrawalBot
          currentExchange={pickedExchange}
          handleReset={resetFormToStep(1)}
          handleSubmit={configureWithdrawalBotHandler}
          getMinimums={getWithdrawalMinimums}
          disable={isCreatingBot}
          errors={errors}
        />
      case 'configure_webhook_bot':
        return <ConfigureWebhookBot
            showLimitOrders={isBasic || isPro || isLegendary}
            currentExchange={pickedExchange}
            handleReset={resetFormToStep(1)}
            handleSubmit={configureWebhookBotHandler}
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
