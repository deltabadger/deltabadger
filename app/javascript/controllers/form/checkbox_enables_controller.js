import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--checkbox-enables"
export default class extends Controller {
  static targets = ["checkbox", "submit"]

  connect() {
    this.toggleSubmitButton()
  }

  checkboxChanged() {
    this.toggleSubmitButton()
  }

  toggleSubmitButton() {
    this.submitTarget.disabled = !this.checkboxTarget.checked
  }
}
