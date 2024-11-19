import "@hotwired/turbo-rails";
import "../controllers";
import React from "react";
import { createRoot } from "react-dom/client";
import I18n from "i18n-js/index.js.erb";
import { NewsletterForm } from "../deltabadger/components/NewsletterForm";

Turbo.setFormMode("off")

document.addEventListener("turbo:load", () => {
  I18n.locale = document.body.dataset.locale || I18n.defaultLocale;
});

if (document.getElementById("newsletter")) {
  document.addEventListener("turbo:load", () => {
    createRoot(document.getElementById("newsletter")).render(
      <NewsletterForm />
    );
  });
}
