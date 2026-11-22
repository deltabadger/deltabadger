import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--password-strength"
export default class extends Controller {
  static targets = ["input", "requirementsList", "length", "uppercase", "lowercase", "digit", "symbol"];
  static values = {
    successClass: String,
    lengthPattern: String,
    uppercasePattern: String,
    lowercasePattern: String,
    digitPattern: String,
    symbolPattern: String
  };

  connect() {
    this.checkPasswordStrength() // Initialize checks if there’s already input
  }

  checkPasswordStrength() {
    const password = this.inputTarget.value

    // The rules open under the field with the first character and stay: only their met state changes after that.
    // (`.reveal` in new/_onboarding_transitions.sass; the markup's `hidden` covers the time before this runs.)
    this.requirementsListTarget.classList.replace("hidden", "reveal")
    this.requirementsListTarget.offsetHeight // the closed state is drawn before it opens
    if (password.length > 0) this.requirementsListTarget.classList.add("is-open")

    this.#updateValidation(this.lengthTarget, this.lengthPatternValue)
    this.#updateValidation(this.uppercaseTarget, this.uppercasePatternValue)
    this.#updateValidation(this.lowercaseTarget, this.lowercasePatternValue)
    this.#updateValidation(this.digitTarget, this.digitPatternValue)
    this.#updateValidation(this.symbolTarget, this.symbolPatternValue)
  }

  #updateValidation(element, regex) {
    const isValid = new RegExp(regex).test(this.inputTarget.value)
    isValid ? element.classList.add(this.successClassValue) : element.classList.remove(this.successClassValue)
  }
}
