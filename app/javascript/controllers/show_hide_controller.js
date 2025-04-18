import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="show-hide"
// Toggles specified classes on target elements when the toggle action is triggered.
//
// Example usage:
// <button data-action="click->show-hide#toggle">Toggle Classes</button>
// <div data-show-hide-target="togglable" data-show-hide-toggle-class-value='["class-one", "class-two"]'>
//   Content with classes to toggle
// </div>
//
export default class extends Controller {
  static targets = ["togglable"]
  static values = { toggleClass: Array }

  toggle() {
    if (!this.hasToggleClassValue) {
      console.error("Missing data-show-hide-toggle-class-value attribute on controller element.");
      return;
    }

    this.togglableTargets.forEach(element => {
      this.toggleClassValue.forEach(klass => {
        element.classList.toggle(klass);
      });
    });
  }
}
