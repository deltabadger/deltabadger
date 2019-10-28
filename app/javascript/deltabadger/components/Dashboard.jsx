import React, { useState, useEffect } from 'react'
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'
import API from '../lib/API'
import { BotForm } from './BotForm'
import { BotDetails } from './BotDetails'
import { Bot } from './Bot'
import { isEmpty, isNotEmpty } from '../utils/array'

export const Dashboard = () => {
  const [bots, setBots] = useState([]);
  const [subscription, setSubscription] = useState({plan: ''});
  const [currentBotId, setCurrentBot] = useState(undefined);
  const currentBot = bots.find(bot => bot.id === currentBotId)

  useEffect(() => {
    checkSubscription()
    loadBots()
  }, []);

  const checkSubscription = () => {
    API.getSubscription().then(({data}) => {
      setSubscription(data)
    })
  }

  const loadBots = (id) => {
    API.getBots().then(({ data }) => {
      const sortedBots = data.sort((a,b) => a.id - b.id)
      setBots(sortedBots)
      id ? openBot(id) : (sortedBots[0] && openBot(sortedBots[0].id))
    })
  }

  const startBot = id => {
    API.startBot(id).then(data => {
      loadBots(id);
    }).catch(() => loadBots())
  }

  const stopBot = id => {
    API.stopBot(id).then(data => {
      loadBots(id);
    }).catch(() => loadBots())
  }

  const removeBot = id => {
    API.removeBot(id).then(data => {
      loadBots();
    }).catch(() => loadBots())
  }

  const openBot = id => setCurrentBot(id)
  const closeAllBots = () => openBot(undefined)

  return (
    <div className="db-bots">
      { bots.map(b =>
        <Bot
          key={`${b.id}-${b.id == currentBot}`}
          bot={b}
          reload={() => { loadBots(currentBotId) }}
          handleStop={stopBot}
          handleStart={startBot}
          handleRemove={removeBot}
          handleClick={openBot}
          open={b.id == currentBotId}
        />
      )}
      <BotForm
        open={isEmpty(bots)}
        currentBot={currentBot}
        callbackAfterCreation={startBot}
        callbackAfterOpening={closeAllBots}
        callbackAfterClosing={() => {bots[0] && openBot(bots[0].id)}}
      />
    </div>
  )
}

