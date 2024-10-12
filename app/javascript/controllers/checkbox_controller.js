import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="checkbox"
export default class extends Controller {
  static targets = ["checkbox", "submit"]

  connect() {
    this.toggleSubmitButton()
  }

  toggleSubmitButton() {
    this.submitTarget.disabled = !this.checkboxTarget.checked
  }

  checkboxChanged() {
    this.toggleSubmitButton()
  }
}
