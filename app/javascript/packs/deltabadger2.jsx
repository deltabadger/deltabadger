import "@hotwired/turbo-rails";
import "../controllers";
import React from "react";
import { createRoot } from "react-dom/client";
import I18n from "i18n-js/index.js.erb";
import * as Sentry from "@sentry/react";
import { Integrations } from "@sentry/tracing";
import { Dashboard } from "../deltabadger/components/Dashboard";
import { Provider } from "react-redux";
import { configureStore } from "../deltabadger/Store";
import { reducer } from "../deltabadger/reducer";

Sentry.init({
  dsn: process.env.REACT_SENTRY_DSN,
  integrations: [new Integrations.BrowserTracing()],
  tracesSampleRate: 1.0,
});

const store = configureStore(reducer);

document.addEventListener("turbo:load", () => {
  I18n.locale = document.body.dataset.locale || I18n.defaultLocale;
});

document.addEventListener("turbo:load", () => {
  const dashboardDiv = document.getElementById("dashboard");
  if (dashboardDiv) {
    const node = document.getElementById("current_user_subscription");
    const data = node ? node.getAttribute("data") : null;
    const isBasic = data === "basic";
    const isPro = data === "pro";
    const isLegendary = data === "legendary";
    const root = createRoot(dashboardDiv);
    root.render(
      <Provider store={store}>
        <Dashboard isBasic={isBasic} isPro={isPro} isLegendary={isLegendary} />
      </Provider>
    );
  }
});

if (document.getElementById("referral_banner_link")) {
  document.addEventListener("turbo:load", () => {
    document
      .getElementById("referral_banner_link")
      .addEventListener("click", (evt) => {
        evt.preventDefault();
        navigator.clipboard
          .writeText(evt.target.getAttribute("href"))
          .then(() => {
            $("#referral_badge").removeClass("invisible");
          });
      });
  });
}

// Update theme color meta tag based on the background color of the body
document.addEventListener("turbo:load", () => {
  function updateThemeColor() {
    const themeColor = getComputedStyle(document.documentElement).getPropertyValue('--background').trim();
    document.getElementById('theme-color-meta').setAttribute('content', themeColor);
  }
  updateThemeColor();
  window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', updateThemeColor);
});
