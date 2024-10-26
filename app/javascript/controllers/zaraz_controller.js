import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="zaraz"
export default class extends Controller {
  static targets = ["button"];
  static values = { currency: String, name: String, price: Number, isPurchase: Boolean };

  connect() {
    if (this.isPurchaseValue) {
      this.#trackPurchaseEvent();
    }
  }

  trackBeginCheckoutEvent(event) {
    const item = {
      name: this.nameValue,
      currency: this.currencyValue,
      quantity: 1,
      price: this.priceValue.toFixed(2)
    };
    const eventProperties = {
      currency: this.currencyValue,
      value: this.priceValue.toFixed(2),
      items: [item]
    };

    zaraz.track("begin_checkout", eventProperties);
    // console.log("event 'begin_checkout' sent:", { currency: this.currencyValue, value: this.priceValue, items: [item] });
  }

  #trackPurchaseEvent() {
    const item = {
      name: this.nameValue,
      currency: this.currencyValue,
      quantity: 1,
      price: this.priceValue.toFixed(2)
    };
    const eventProperties = {
      currency: this.currencyValue,
      value: this.priceValue.toFixed(2),
      items: [item]
    };

    zaraz.track("purchase", eventProperties);
    // console.log("event 'purchase' sent:", { currency: this.currencyValue, value: this.priceValue, items: [item] });
  }
}
