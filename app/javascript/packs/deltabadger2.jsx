import Rails from 'rails-ujs';
import React from 'react';
import ReactDOM from 'react-dom';
import { Dashboard } from '../deltabadger/components/Dashboard';
import style from '../deltabadger/styles/main.scss';
import { CookieBanner } from '../deltabadger/components/CookieBanner';
import { Provider } from 'react-redux'
import { configureStore } from '../deltabadger/Store'
import { reducer } from '../deltabadger/reducer'

require.context('../images', true)

Rails.start();

const store = configureStore(reducer)

if (document.getElementById('dashboard')) {
  document.addEventListener('DOMContentLoaded', () => {
    ReactDOM.render(
      <Provider store={store}>
        <Dashboard />
      </Provider>,
      document.getElementById('dashboard')
    )
  })
}

if (document.getElementById('cookie_consent')) {
  document.addEventListener('DOMContentLoaded', () => {
    ReactDOM.render(
      <CookieBanner />,
      document.getElementById('cookie_consent')
    )
  })
}

if (document.getElementById('hide_welcome_banner_button')) {
  document.addEventListener('DOMContentLoaded', () => {
    document
      .getElementById('hide_welcome_banner_button')
      .addEventListener("click", () => {
        Rails.ajax({
          url: "/settings/hide_welcome_banner",
          type: "patch"
        });
      });
  })
}
