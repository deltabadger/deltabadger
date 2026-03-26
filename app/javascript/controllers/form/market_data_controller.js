import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--market-data"
export default class extends Controller {
  static targets = ["coingeckoFields", "coingeckoButtons", "deltabadgerButtons"]

  selectNone(event) {
    this.#hideCoinGeckoForm()
    this.#hideDeltabadgerButtons()
    this.element.requestSubmit()
  }

  selectCoinGecko(event) {
    this.#hideDeltabadgerButtons()
    this.element.requestSubmit()
  }

  selectDeltabadger(event) {
    this.#hideCoinGeckoForm()
    this.element.requestSubmit()
  }

  #hideCoinGeckoForm() {
    if (this.hasCoinGeckoFieldsTarget) this.coingeckoFieldsTarget.style.display = 'none'
    if (this.hasCoinGeckoButtonsTarget) this.coingeckoButtonsTarget.style.display = 'none'
  }

  #hideDeltabadgerButtons() {
    if (this.hasDeltabadgerButtonsTarget) this.deltabadgerButtonsTarget.style.display = 'none'
  }
}
