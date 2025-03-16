import { Controller } from "@hotwired/stimulus";
import flatpickr from "flatpickr";
import { english } from "flatpickr/dist/l10n/default.js";
import { German } from "flatpickr/dist/l10n/de.js";
import { Dutch } from "flatpickr/dist/l10n/nl.js";
import { French } from "flatpickr/dist/l10n/fr.js";
import { Spanish } from "flatpickr/dist/l10n/es.js";
import { Italian } from "flatpickr/dist/l10n/it.js";
import { Catalan } from "flatpickr/dist/l10n/cat.js";
import { Portuguese } from "flatpickr/dist/l10n/pt.js";
import { Polish } from "flatpickr/dist/l10n/pl.js";
import { Russian } from "flatpickr/dist/l10n/ru.js";
import { Indonesian } from "flatpickr/dist/l10n/id.js";
import { Vietnamese } from "flatpickr/dist/l10n/vn.js";
import { Mandarin } from "flatpickr/dist/l10n/zh.js";
require("flatpickr/dist/flatpickr.css");
// require("flatpickr/dist/themes/dark.css");

// Connects to data-controller="flatpickr"
export default class extends Controller {
  static values = { locale: String, maxDate: String, minDate: String };

  locales = {
    en: english,
    de: German,
    nl: Dutch,
    fr: French,
    es: Spanish,
    it: Italian,
    ca: Catalan,
    pt: Portuguese,
    pl: Polish,
    ru: Russian,
  };

  connect() {
    flatpickr(".fp_date_time", {
      locale: this.locales[this.localeValue],
      enableTime: true,
      dateFormat: "Y-m-d H:i",
    });

    flatpickr(".fp_date", {
      locale: this.locales[this.localeValue],
      maxDate: this.maxDateValue,
      position: "auto center",
    });
  }
}
