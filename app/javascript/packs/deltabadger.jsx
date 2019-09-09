import Rails from 'rails-ujs';
import React from 'react'
import ReactDOM from 'react-dom'
import { Dashboard } from '../deltabadger/components/Dashboard'

Rails.start();

if (document.getElementById('dashboard')) {
  document.addEventListener('DOMContentLoaded', () => {
    ReactDOM.render(
      <Dashboard />,
      document.getElementById('dashboard')
    )
  })
}
