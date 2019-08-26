import React from 'react'
import ReactDOM from 'react-dom'
import { Dashboard } from '../deltabadger/components/Dashboard'

if (document.getElementById('dashboard')) {
  document.addEventListener('DOMContentLoaded', () => {
    ReactDOM.render(
      <Dashboard />,
      document.getElementById('dashboard')
    )
  })
}
