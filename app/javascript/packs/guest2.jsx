import React from 'react'
import ReactDOM from 'react-dom'
import Rails from 'rails-ujs';
import style from '../deltabadger/styles/main.scss'
import { CookieBanner } from '../deltabadger/components/CookieBanner';

require.context('../images', true)

Rails.start();

if (document.getElementById('cookie_consent')) {
  document.addEventListener('DOMContentLoaded', () => {
    ReactDOM.render(
      <CookieBanner />,
      document.getElementById('cookie_consent')
    )
  })
}
