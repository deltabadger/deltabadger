import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--checkbox-enables"
export default class extends Controller {
  static targets = ["checkbox", "submit"]

  connect() {
    console.log("checkbox-enables connected")
  }

  toggleSubmitButton() {
    const anyChecked = this.checkboxTargets.some(checkbox => checkbox.checked)
    this.submitTarget.disabled = !anyChecked
  }
}
