import React, { useState, useEffect } from 'react'
import ReactDOM from 'react-dom'
import PropTypes from 'prop-types'
import API from '../lib/API'
import { BotForm } from './BotForm'


const Bot = ({ settings, exchangeName }) => {
  const description = `${settings.type} for ${settings.price}${settings.currency}/${settings.interval} on ${exchangeName}`
  return (
    <div>
      { description }
    </div>
  )
}

export const Dashboard = () => {
  const [bots, setBots] = useState([]);

  useEffect(() => {
    bots.length == 0 && API.getBots().then(data => {
      setBots(data.data)
    })
  }, []);

  const callbackAfterCreation = () => {
    API.getBots().then(data => {
      setBots(data.data)
    })
  }

  return (
    <div>
      <h1>Dashboard</h1>
        { bots.map(b => <Bot key={b.id} settings={b.settings} exchangeName={b.exchangeName}/>) }
      <BotForm callbackAfterCreation={callbackAfterCreation} />
    </div>
  )
}

