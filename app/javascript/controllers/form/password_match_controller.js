import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--password-match"
export default class extends Controller {
  static targets = ["input0", "input1", "match"];
  static values = { successClass: String };

  connect() {
    this.checkPasswordMatch() // Initialize checks if there’s already input
  }

  checkPasswordMatch() {
    const password = this.input0Target.value
    const password_confirmation = this.input1Target.value

    if (password_confirmation.length > 0) {
      this.matchTarget.classList.remove("hidden")
    } else {
      this.matchTarget.classList.add("hidden")
    }

    const isValid = password === password_confirmation
    isValid ? this.matchTarget.classList.add(this.successClassValue) : this.matchTarget.classList.remove(this.successClassValue)
  }
}
