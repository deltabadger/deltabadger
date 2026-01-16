import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="value-toggle"
// Toggles visibility between two value displays on click.
//
// Example usage:
// <div data-controller="value-toggle" data-action="click->value-toggle#toggle">
//   <span data-value-toggle-target="primary">+15.00%</span>
//   <span data-value-toggle-target="secondary" class="d-none">+$1,500</span>
// </div>
//
export default class extends Controller {
  static targets = ["primary", "secondary"]

  toggle() {
    this.primaryTarget.classList.toggle("d-none")
    this.secondaryTarget.classList.toggle("d-none")
  }
}
