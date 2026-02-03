import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--market-data"
export default class extends Controller {
  static targets = ["coingeckoFields", "coingeckoButtons", "deltabadgerButtons"]

  selectNone(event) {
    this.#hideCoingeckoForm()
    this.#hideDeltabadgerButtons()
    this.element.requestSubmit()
  }

  selectCoingecko(event) {
    this.#hideDeltabadgerButtons()
    this.element.requestSubmit()
  }

  selectDeltabadger(event) {
    this.#hideCoingeckoForm()
    this.element.requestSubmit()
  }

  #hideCoingeckoForm() {
    if (this.hasCoingeckoFieldsTarget) this.coingeckoFieldsTarget.style.display = 'none'
    if (this.hasCoingeckoButtonsTarget) this.coingeckoButtonsTarget.style.display = 'none'
  }

  #hideDeltabadgerButtons() {
    if (this.hasDeltabadgerButtonsTarget) this.deltabadgerButtonsTarget.style.display = 'none'
  }
}
