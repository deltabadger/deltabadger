import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--email-notifications"
export default class extends Controller {
  static targets = ["smtpFields", "smtpButtons", "envButtons"]

  selectNone(event) {
    this.#hideSmtpForm()
    this.element.requestSubmit()
  }

  selectCustom(event) {
    this.#hideEnvButtons()
    this.element.requestSubmit()
  }

  selectEnv(event) {
    this.#hideSmtpForm()
    this.element.requestSubmit()
  }

  #hideSmtpForm() {
    if (this.hasSmtpFieldsTarget) this.smtpFieldsTarget.style.display = 'none'
    if (this.hasSmtpButtonsTarget) this.smtpButtonsTarget.style.display = 'none'
  }

  #hideEnvButtons() {
    if (this.hasEnvButtonsTarget) this.envButtonsTarget.style.display = 'none'
  }
}
