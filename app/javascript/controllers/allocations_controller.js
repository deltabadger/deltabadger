import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="allocations"
export default class extends Controller {
  static targets = ["allocation"]

  connect() {
    this.updateAllTargets(); // Optional, to synchronize on load
  }

  update(event) {
    const newValue = event.target.value;
    this.allocationTargets.forEach((target) => {
      if (target !== event.target) {
        target.value = newValue;
      }
    });
  }

  updateAllTargets() {
    const initialValue = this.allocationTargets[0]?.value;
    this.allocationTargets.forEach((target) => {
      target.value = initialValue;
    });
  }
}