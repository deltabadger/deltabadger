import { Controller } from "@hotwired/stimulus";
import debounce from "debounce";

// Automatically submits a form after a delay

// Connects to data-controller="form--submit-after-delay"
export default class extends Controller {
  initialize() {
    this.submit = debounce(this.submit.bind(this), 250);
  }

  submit() {
    const inputElements = this.element.querySelectorAll("input");
    let someNumericInputHasToBeFilledBeforeSubmit = false;

    inputElements.forEach((inputElement) => {
      const value = inputElement.value;

      if (inputElement.type === "number") {
        // Check if the value ends with a comma, period, or zero after a comma or period
        someNumericInputHasToBeFilledBeforeSubmit = /^\d+[,.](0*|\d*0)$/.test(value);
      }
    });

    if (!someNumericInputHasToBeFilledBeforeSubmit) {
      this.element.requestSubmit();
    }
  }
}
