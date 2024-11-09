import { Controller } from "@hotwired/stimulus"

// works in combination with css :not([value=""])
// Connects to data-controller="form--label-animations"
export default class extends Controller {
  connect() {
    this.element.querySelectorAll("input, textarea").forEach((input) => {
      if (!input.hasAttribute("value")) {
        input.setAttribute("value", "");
      }
      input.addEventListener("change", this.#updateValue.bind(this));
    });
  }

  #updateValue(event) {
    const input = event.target;
    if (input.type === "password") {
      input.setAttribute("value", "•".repeat(input.value.length));
    } else {
      input.setAttribute("value", input.value);
    }
  }
}