import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="allocations"
export default class extends Controller {
  static targets = ["allocation"]

  update(event) {
    const newValue = event.target.value;
    this.allocationTargets.forEach((target) => {
      if (target !== event.target) {
        target.value = newValue;
      }
    });
  }
}