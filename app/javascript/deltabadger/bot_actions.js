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

const botReloaded = (bot) => ({
  type: 'BOT_RELOADED',
  payload: bot
})

const botRemoved = (id) => ({
  type: 'REMOVE_BOT',
  payload: id
})

const setErrors = ({id, errors}) => ({ type: 'SET_ERRORS', payload: {[id]: errors}})
const cleanErrors = (id) => ({ type: 'SET_ERRORS', payload: {[id]: []}})

export const loadBots = (openFirstBot = false) => dispatch => {
  return API.getBots().then(({ data }) => {
    const sortedBots = data.sort((a,b) => a.id - b.id)
    dispatch(fetchedBots(sortedBots));
    if (openFirstBot) { dispatch(openBot(sortedBots[0].id)) }
  })
}

export const removeBot = id => (dispatch) => {
  return API.removeBot(id).then(data => {
    dispatch(botRemoved(id));
  })
}

export const startBot = (id) => dispatch => {
  API.startBot(id).then(({data: bot}) => {
    dispatch(cleanErrors(bot.id))
    dispatch(botReloaded(bot))
    dispatch(openBot(bot.id))
  }).catch((data) => {
    dispatch(setErrors(data.response.data))
    dispatch(openBot(id))
  })
}

export const stopBot = (id) => dispatch => {
  API.stopBot(id).then(({data: bot}) => {
    dispatch(botReloaded(bot))
  })
}


let timeout = (callback) => setTimeout(() => {
  callback()
}, 2000)


export const reloadBot = (currentBot) => dispatch => {
  API.getBot(currentBot.id).then(({data: reloadedBot}) => {
    if (currentBot.nextTransactionTimestamp != reloadedBot.nextTransactionTimestamp) {
      clearTimeout(timeout)
      dispatch(botReloaded(reloadedBot))
    } else {
      timeout(() => reloadBot(currentBot))
    }
  })
}

export const editBot = botParams => dispatch => {
  API.updateBot(botParams).then(({data: bot}) => {
    dispatch(cleanErrors(bot.id))
    dispatch(startBot(bot.id))
  }).catch((data) => {
    dispatch(setErrors(data.response.data))
  })
}
