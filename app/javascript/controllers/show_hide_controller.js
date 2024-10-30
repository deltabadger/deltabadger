import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="show-hide"
export default class extends Controller {
  static targets = ["hideable", "toggleButton"]

  toggle() {
    // Toggle visibility of elements with the hideable target
    this.hideableTargets.forEach(element => element.classList.toggle("hidden"));

    // Toggle the active class on the toggleButton if it exists
    if (this.hasToggleButtonTarget) {
      this.toggleButtonTarget.classList.toggle("active");
    }
  }
}
