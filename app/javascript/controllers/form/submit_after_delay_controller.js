import { Controller } from "@hotwired/stimulus";
import debounce from "debounce";

// Automatically submits a form after a delay

// Connects to data-controller="form--submit-after-delay"
export default class extends Controller {
  // What each field read at its last input event. A field not typed in yet reads what the server drew.
  #seen = new WeakMap();

  initialize() {
    this.submitLater = debounce(this.#submit.bind(this), 250);
  }

  disconnect() {
    this.submitLater.clear();
  }

  submit(event) {
    const field = event?.target;
    const before = this.#seen.has(field) ? this.#seen.get(field) : field?.defaultValue;
    if (field) this.#seen.set(field, field.value);
    // Chrome reads a number field holding "0." as "0": a separator typed at the end leaves the value as it was.
    // Submitting it would answer with the form drawn again from "0", and the separator would be gone, so it waits
    // for the digits after it. A separator that changes the value ("15" into "1.5") is a finished edit.
    if ((event?.data === "." || event?.data === ",") && field?.value === before) {
      this.submitLater.clear();
      return;
    }
    this.submitLater();
  }

  #submit() {
    const inputElements = this.element.querySelectorAll("input");
    let someNumericInputHasToBeFilledBeforeSubmit = false;

    for (const inputElement of inputElements) {
      const value = inputElement.value;

      if (inputElement.type === "number") {
        // Check if the value is empty, ends with a comma, period, or zero after a comma or period
        someNumericInputHasToBeFilledBeforeSubmit = /^(?:\d+[,.](?:0*|\d*0)|)$/.test(value);
        if (someNumericInputHasToBeFilledBeforeSubmit) break;
      }
    }

    if (!someNumericInputHasToBeFilledBeforeSubmit) {
      this.element.requestSubmit();
    }
  }
}
