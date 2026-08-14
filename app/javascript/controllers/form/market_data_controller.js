import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--market-data"
export default class extends Controller {
  static targets = ["providerForm", "coingeckoFields", "coingeckoButtons", "deltabadgerButtons"]

  selectNone(event) {
    this.#hideCoinGeckoForm()
    this.#hideDeltabadgerButtons()
    this.providerFormTarget.requestSubmit()
  }

  selectCoinGecko(event) {
    this.#hideDeltabadgerButtons()
    this.providerFormTarget.requestSubmit()
  }

  selectDeltabadger(event) {
    this.#hideCoinGeckoForm()
    if (this.hasDeltabadgerButtonsTarget) this.deltabadgerButtonsTarget.style.display = 'flex'
  }

  selectConnectedDeltabadger(event) {
    this.#hideCoinGeckoForm()
    this.providerFormTarget.requestSubmit()
  }

  #hideCoinGeckoForm() {
    if (this.hasCoinGeckoFieldsTarget) this.coingeckoFieldsTarget.style.display = 'none'
    if (this.hasCoinGeckoButtonsTarget) this.coingeckoButtonsTarget.style.display = 'none'
  }

  #hideDeltabadgerButtons() {
    if (this.hasDeltabadgerButtonsTarget) this.deltabadgerButtonsTarget.style.display = 'none'
  }
}
