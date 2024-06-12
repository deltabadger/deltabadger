import "@hotwired/turbo-rails";
import "../controllers";
import React from "react";
import { createRoot } from "react-dom/client";
import I18n from "i18n-js/index.js.erb";
import style from "../deltabadger/styles/main.scss";
import { CookieBanner } from "../deltabadger/components/CookieBanner";
import { NewsletterForm } from "../deltabadger/components/NewsletterForm";

require.context("../images", true);

I18n.locale = document.head.dataset.locale || I18n.defaultLocale;

if (document.getElementById("cookie_consent")) {
  document.addEventListener("turbo:load", () => {
    createRoot(document.getElementById("cookie_consent")).render(
      <CookieBanner />
    );
  });
}

if (document.getElementById("newsletter")) {
  document.addEventListener("turbo:load", () => {
    createRoot(document.getElementById("newsletter")).render(
      <NewsletterForm />
    );
  });
}
