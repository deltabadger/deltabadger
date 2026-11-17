import { Controller } from "@hotwired/stimulus"

// The market data widget's provider switch.
//
// The segmented control announces the provider as `segmented:change`; this puts it in the switch
// form's hidden field and submits, so the server makes the switch and re-renders the widget in
// whatever state that provider needs — CoinGecko without a key comes back with its connect card.
// The one provider the server cannot switch to on its own is an unclaimed Deltabadger.com: that
// needs a connect code first, so its form is shown instead.
//
// Connects to data-controller="form--market-data" on the widget.
export default class extends Controller {
  static targets = ["providerForm", "provider", "coingeckoFields", "deltabadgerButtons"]
  static values = { platformConnected: Boolean }

  select(event) {
    const provider = event.detail.value
    this.providerTarget.value = provider
    // Every element of a provider's (the wizard draws its Connect apart from its fields).
    this.coingeckoFieldsTargets.forEach((element) => { element.hidden = provider !== "coingecko" })
    this.deltabadgerButtonsTargets.forEach((element) => { element.hidden = provider !== "deltabadger" })
    if (provider === "deltabadger" && !this.platformConnectedValue) return

    this.providerFormTarget.requestSubmit()
  }
}
