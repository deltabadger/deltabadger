import React from 'react'
import ReactDOM from 'react-dom'
import { BotForm } from '../deltabadger/components/BotForm'

document.addEventListener('DOMContentLoaded', () => {
  ReactDOM.render(
    <BotForm />,
    document.getElementById('bot_form')
  )
})
