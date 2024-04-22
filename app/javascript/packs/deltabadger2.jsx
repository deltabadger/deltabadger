import "@hotwired/turbo-rails"
import React from 'react';
import { createRoot } from 'react-dom/client';
import I18n from 'i18n-js/index.js.erb';
import * as Sentry from "@sentry/react";
import { Integrations } from "@sentry/tracing";
import { Dashboard } from '../deltabadger/components/Dashboard';
import style from '../deltabadger/styles/main.scss';
import { CookieBanner } from '../deltabadger/components/CookieBanner';
import { Provider } from 'react-redux'
import { configureStore } from '../deltabadger/Store'
import { reducer } from '../deltabadger/reducer'
import { Chart, Tooltip} from 'chart.js/auto';
import 'chartjs-adapter-date-fns';
window.Chart = Chart;
window.Tooltip = Tooltip;

Sentry.init({
  dsn: process.env.REACT_SENTRY_DSN,
  integrations: [
    new Integrations.BrowserTracing(),
  ],
  tracesSampleRate: 1.0,
});

require.context('../images', true)

const store = configureStore(reducer)

I18n.locale = document.head.dataset.locale || I18n.defaultLocale

document.addEventListener('turbo:load', () => {
  const dashboardDiv = document.getElementById('dashboard');
  if (dashboardDiv) {
    const node = document.getElementById('current_user_subscription');
    const data = node ? node.getAttribute('data') : null;
    const isHodler = data === 'hodler';
    const isLegendaryBadger = data === 'legendary_badger';
    const root = createRoot(dashboardDiv);
    root.render(
      <Provider store={store}>
        <Dashboard isHodler={isHodler} isLegendaryBadger={isLegendaryBadger} />
      </Provider>
    );
  }
})

if (document.getElementById('cookie_consent')) {
  document.addEventListener('DOMContentLoaded', () => {
    createRoot(
      document.getElementById('cookie_consent')
    ).render(
      <CookieBanner />
    )
  })
}

// if (document.getElementById('hide_welcome_banner_button')) {
//   document.addEventListener('DOMContentLoaded', () => {
//     document
//       .getElementById('hide_welcome_banner_button')
//       .addEventListener("click", () => {
//         Rails.ajax({
//           url: "/settings/hide_welcome_banner",
//           type: "patch"
//         });
//       });
//   })
// }

// if (document.getElementById('hide_referral_banner_button')) {
//   document.addEventListener('DOMContentLoaded', () => {
//     document
//       .getElementById('hide_referral_banner_button')
//       .addEventListener("click", () => {
//         Rails.ajax({
//           url: "/settings/hide_referral_banner",
//           type: "patch"
//         });
//       });
//   })
// }

if (document.getElementById('referral_banner_link')) {
  document.addEventListener('DOMContentLoaded', () => {
    document
      .getElementById('referral_banner_link')
      .addEventListener("click", (evt) => {
        evt.preventDefault();
        navigator.clipboard.writeText(evt.target.getAttribute('href')).then(() => {
            $('#referral_badge').removeClass('invisible')
        })
      });
  })
}
