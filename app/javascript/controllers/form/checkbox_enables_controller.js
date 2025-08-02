import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--checkbox-enables"
export default class extends Controller {
  static targets = ["checkbox", "submit"]
  static values = { mode: { type: String, default: 'some' } } // 'some' or 'all'

  connect() {
    // console.log("checkbox-enables connected")
  }

  toggleSubmitButton() {
    let shouldEnable = false;
    if (this.modeValue === 'all') {
      shouldEnable = this.checkboxTargets.every(checkbox => checkbox.checked);
    } else {
      shouldEnable = this.checkboxTargets.some(checkbox => checkbox.checked);
    }
    this.submitTarget.disabled = !shouldEnable;
  }
}
