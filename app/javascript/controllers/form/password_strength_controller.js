import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--password-strength"
export default class extends Controller {
  static targets = ["input", "requirementsList", "length", "uppercase", "lowercase", "digit", "symbol"];
  static values = {
    successColor: String,
    labelColor: String,
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

    if (password.length > 0) {
      this.requirementsListTarget.classList.remove("hidden")
    } else {
      this.requirementsListTarget.classList.add("hidden")
    }

    this.updateValidation(this.lengthTarget, this.lengthPatternValue)
    this.updateValidation(this.uppercaseTarget, this.uppercasePatternValue)
    this.updateValidation(this.lowercaseTarget, this.lowercasePatternValue)
    this.updateValidation(this.digitTarget, this.digitPatternValue)
    this.updateValidation(this.symbolTarget, this.symbolPatternValue)
  }

  updateValidation(element, regex) {
    const isValid = new RegExp(regex).test(this.inputTarget.value)
    element.style.color = isValid ? this.successColorValue : this.labelColorValue
  }
}
