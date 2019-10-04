import Rails from 'rails-ujs';
import React from 'react'
import ReactDOM from 'react-dom'
import style from '../deltabadger/styles/main.scss'
import { CookieBanner } from '../deltabadger/components/CookieBanner';
import { NewsletterForm } from '../deltabadger/components/NewsletterForm';

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

if (document.getElementById('newsletter')) {
  document.addEventListener('DOMContentLoaded', () => {
    ReactDOM.render(
      <NewsletterForm />,
      document.getElementById('newsletter')
    )
  })
}
