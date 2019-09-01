import React, { useState, useEffect } from 'react'
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'
import API from '../lib/API'
import { BotForm } from './BotForm'


const Bot = ({ id, settings, status, exchangeName, handleStart, handleStop }) => {
  const description = `${settings.type} for ${settings.price}${settings.currency}/${settings.interval} on ${exchangeName}`

  const StartButton = () => (<button onClick={() => handleStart(id)}>Start</button>)
  const StopButton = () => (<button onClick={() => handleStop(id)}>Stop</button>)

  return (
    <div>
      { description }
      { status == 'working' ? <StopButton /> : <StartButton/> }
    </div>
  )
}

export const Dashboard = () => {
  const [bots, setBots] = useState([]);

  const loadBots = () => {
    API.getBots().then(data => {
      setBots(data.data)
    })
  }

  useEffect(() => {
    loadBots()
  }, []);

  const callbackAfterCreation = () => {
    loadBots()
  }

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

  return (
    <div>
      <h1>Dashboard</h1>
      { bots.map(b =>
        <Bot
          id={b.id}
          key={b.id}
          status={b.status}
          settings={b.settings}
          exchangeName={b.exchangeName}
          handleStop={stopBot}
          handleStart={startBot}
        />)
      }
      <BotForm callbackAfterCreation={callbackAfterCreation} />
    </div>
  )
}

