import React from 'react'
import ReactDOM from 'react-dom'
import { BotForm } from '../deltabadger/components/BotForm'

if (document.getElementById('bot_form')) {
  document.addEventListener('DOMContentLoaded', () => {
    ReactDOM.render(
      <BotForm />,
      document.getElementById('bot_form')
    )
  })
}
