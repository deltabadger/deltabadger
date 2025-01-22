import "@hotwired/turbo-rails";
import "../controllers";
import React from "react";
import { createRoot } from "react-dom/client";
import I18n from "i18n-js/index.js.erb";

Turbo.setFormMode("off")

document.addEventListener("turbo:load", () => {
  I18n.locale = document.body.dataset.locale || I18n.defaultLocale;
});
