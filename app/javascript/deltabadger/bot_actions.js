import API from './lib/API';

export const openBot = (id) => ({
  type: 'SET_CURRENT_BOT',
  payload: id
})

export const closeAllBots = () => ({
  type: 'CLOSE_ALL_BOTS',
})

const fetchedBots = (bots) => ({
  type: 'FETCHED_BOTS',
  payload: bots
})

const tryStartBot = (id) => ({
  type: 'TRY_START_BOT',
  payload: id
})

const botReloaded = (bot) => ({
  type: 'BOT_RELOADED',
  payload: bot
})

const botRemoved = (id) => ({
  type: 'REMOVE_BOT',
  payload: id
})

const fetchedMinimums = (data) => ({
  type: 'MINIMUMS_FETCHED',
  payload: data
})

const fetchedNumberOfPages = (numberOfPages) => ({
  type: 'FETCHED_NUMBER_OF_PAGES',
  payload: numberOfPages
})

const setErrors = ({id, errors}) => ({ type: 'SET_ERRORS', payload: {[id]: errors}})

export const clearErrors = (id) => setErrors({ id, errors: [] })

export const loadBots = (openFirstBot = false, page) => dispatch => {
  return API.getBots(page).then(({ data }) => {
    if (data.bots.length === 0) { return }

    const numberOfPages = data.number_of_pages
    dispatch(fetchedBots(data.bots))
    dispatch(fetchedNumberOfPages(numberOfPages))
    if (openFirstBot) { dispatch(openBot(data.bots[0].id)) }
  })
}

export const removeBot = id => (dispatch) => {
  return API.removeBot(id).then(_data => {
    dispatch(botRemoved(id))
  })
}

export const startBot = (id, continueParams = null) => dispatch => {
  dispatch(tryStartBot(id))

  if (continueParams === null) {
    continueParams = {continueSchedule: false, price: null}
  }

  API.startBot({id: id, continueParams: continueParams}).then(({data: bot}) => {
    dispatch(clearErrors(bot.id))
    dispatch(botReloaded(bot))
    dispatch(openBot(bot.id))
  }).catch((data) => {
    dispatch(fetchBot(id))
    dispatch(setErrors(data.response.data))
    dispatch(openBot(id))
  })
}

export const stopBot = (id) => dispatch => {
  API.stopBot(id).then(({data: bot}) => {
    dispatch(botReloaded(bot))
  })
}

export const fetchRestartParams = (id) => dispatch => (
  API.fetchRestartParams(id).then((data) => {
    return data
  })
)

export const getSmartIntervalsInfo = (botParams) => dispatch => {
  return API.getSmartIntervalsInfo(botParams).then((data) => {
    dispatch(fetchedMinimums(data))
    return data
  }).catch((data) => {
    dispatch(fetchedMinimums({data: {showSmartIntervalsInfo: false}}))
  })
}

export const setShowSmartIntervalsInfo = () => {
  API.setShowSmartIntervalsInfo().then(data => data)

}

let timeout = (callback) => setTimeout(() => {
  callback()
}, 2000)


export const reloadBot = (currentBot) => dispatch => {
  API.getBot(currentBot.id).then(({data: reloadedBot}) => {
    if (currentBot.nextTransactionTimestamp != reloadedBot.nextTransactionTimestamp ||
      currentBot.nextResultFetchingTimestamp != reloadedBot.nextResultFetchingTimestamp) {
      clearTimeout(timeout)
      dispatch(botReloaded(reloadedBot))
    } else {
      timeout(() => reloadBot(currentBot))
    }
  })
}

export const fetchBot = (id) => dispatch => {
  API.getBot(id).then(({data: bot}) =>
    dispatch(botReloaded(bot))
  )
}

export const editBot = (botParams, continueParams) => dispatch => {
  API.updateBot(botParams).then(({data: bot}) => {
    dispatch(clearErrors(bot.id))
    dispatch(startBot(bot.id, continueParams))
  }).catch((data) => {
    dispatch(setErrors(data.response.data))
  })
}
