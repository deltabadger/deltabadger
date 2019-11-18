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

  const updateBot = (bot) => {
    const newBots = bots.map(b => b.id == bot.id ? bot : b)
    setBots(newBots)
  }

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

  const removeBot = id => {
    API.removeBot(id).then(data => {
      loadBots();
    }).catch(() => loadBots())
  }

  const startBot = id => {
    API.startBot(id).then(({data: bot}) => {
      updateBot(bot)
      openBot(id)
    }).catch(() => loadBots())
  }

  const stopBot = id => {
    API.stopBot(id).then(({data: bot}) => {
      updateBot(bot)
      openBot(id)
    })
  }

  const reloadBot = (currentBot) => {
    API.getBot(currentBot.id).then(({data: reloadedBot}) => {
      if (currentBot.nextTransactionTimestamp != reloadedBot.nextTransactionTimestamp) {
        updateBot(reloadedBot)
      } else {
        setTimeout(() => {
          reloadBot(reloadedBot)
        }, 2000)
      }
    })
  }

  const editBot = (botParams) => {
    API.updateBot(botParams).then(({data: bot}) => {
      startBot(bot.id)
    })
  }

  const buildBotsList = (botsToRender, b) => {
    botsToRender.push(
      <Bot
        key={`${b.id}-${b.id == currentBot}`}
        bot={b}
        reload={reloadBot}
        handleStop={stopBot}
        handleStart={startBot}
        handleRemove={removeBot}
        handleEdit={editBot}
        handleClick={openBot}
        open={b.id == currentBotId}
      />
    )

    if (b.id == currentBotId) botsToRender.push(
      <BotDetails key={`${b.id}-details${b.id == currentBot}`} bot={b}/>
    )

    return botsToRender
  }

  const openBot = id => setCurrentBot(id)
  const closeAllBots = () => openBot(undefined)

  return (
    <div className="db-bots">
      { bots.reduce(buildBotsList, []) }
      <BotForm
        open={isEmpty(bots)}
        currentBot={currentBot}
        callbackAfterCreation={(id) => {
          loadBots(id)
          startBot(id)
        }}
        callbackAfterOpening={closeAllBots}
        callbackAfterClosing={() => {bots[0] && openBot(bots[0].id)}}
      />
    </div>
  )
}

