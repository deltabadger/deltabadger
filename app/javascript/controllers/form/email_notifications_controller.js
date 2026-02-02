import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--email-notifications"
export default class extends Controller {
  static targets = ["gmailFields", "gmailButtons", "envButtons"]

  selectNone(event) {
    this.#hideGmailForm()
    this.element.requestSubmit()
  }

  selectGmail(event) {
    this.#hideEnvButtons()
    this.element.requestSubmit()
  }

  selectEnv(event) {
    this.#hideGmailForm()
    this.element.requestSubmit()
  }

  #hideGmailForm() {
    if (this.hasGmailFieldsTarget) this.gmailFieldsTarget.style.display = 'none'
    if (this.hasGmailButtonsTarget) this.gmailButtonsTarget.style.display = 'none'
  }

  #hideEnvButtons() {
    if (this.hasEnvButtonsTarget) this.envButtonsTarget.style.display = 'none'
  }
}
