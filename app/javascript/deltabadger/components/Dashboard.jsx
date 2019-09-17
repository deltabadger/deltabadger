import React, { useState, useEffect } from 'react'
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'
import API from '../lib/API'
import { BotForm } from './BotForm'
import { BotDetails } from './BotDetails'
import { Bot } from './Bot'

export const Dashboard = () => {
  const [bots, setBots] = useState([]);
  const [currentBotId, setCurrentBot] = useState(undefined);
  const currentBot = bots.find(bot => bot.id === currentBotId)

  useEffect(() => {
    loadBots()
  }, []);

  const loadBots = () => {
    API.getBots().then(({ data }) => {
      setBots(data.sort((a,b) => a.id - b.id))
    })
  }

  const callbackAfterCreation = () => loadBots()

  const startBot = id => {
    API.startBot(id).then(data => {
      loadBots();
    })
  }

  const stopBot = id => {
    API.stopBot(id).then(data => {
      loadBots();
    })
  }

  const openBot = id => setCurrentBot(id)

  return (
    <div className="db-bots">
      { bots.map(b =>
        <Bot
          id={b.id}
          key={`${b.id}-${b.id == currentBot}`}
          status={b.status}
          settings={b.settings}
          exchangeName={b.exchangeName}
          handleStop={stopBot}
          handleStart={startBot}
          handleClick={openBot}
          open={b.id == currentBotId}
        />)
      }
      <BotForm callbackAfterCreation={callbackAfterCreation} />
      { currentBot && <BotDetails bot={currentBot} /> }
    </div>
  )
}

