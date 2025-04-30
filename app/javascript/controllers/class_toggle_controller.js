import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="class-toggle"
// Toggles specified classes on target elements when the toggle action is triggered.
//
// Example usage:
// <button data-action="click->class-toggle#toggle">Toggle Classes</button>
// <div data-class-toggle-target="togglable" data-class-toggle-toggle-classes-value='["class-one", "class-two"]'>
//   Content with classes to toggle
// </div>
//
export default class extends Controller {
  static targets = ["togglable"]
  static values = { toggleClasses: Array }

  toggle() {
    if (!this.hasToggleClassesValue) {
      console.error("Missing data-class-toggle-toggle-classes-value attribute on controller element.");
      return;
    }

    this.togglableTargets.forEach(element => {
      this.toggleClassesValue.forEach(toggleClass => {
        element.classList.toggle(toggleClass);
      });
    });
  }
}
