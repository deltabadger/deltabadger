import React, { useState, useEffect } from 'react'
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'
import API from '../lib/API'
import { BotForm } from './BotForm'
import { BotDetails } from './BotDetails'
import { Bot } from './Bot'

export const Dashboard = () => {
  const [bots, setBots] = useState([]);
  const [currentBot, setCurrentBot] = useState(undefined);

  const loadBots = () => {
    API.getBots().then(data => {
      setBots(data.data)
    })
  }

  useEffect(() => {
    loadBots()
  }, []);

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
          open={b.id == currentBot}
        />)
      }
      <BotForm callbackAfterCreation={callbackAfterCreation} />
    </div>
  )
}

