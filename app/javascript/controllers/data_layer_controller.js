import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="data-layer"
// workaround when using current jQuery implementation
export default class extends Controller {
  static targets = ["button"];

  sendBeginCheckoutEvent(event) {
    event.preventDefault()
    let name = undefined;
    let currency = undefined;
    let price = undefined;
    const button = document.querySelector("button.btn.btn-success");
    const children = button.children;
    for (let i = 0; i < children.length; i++) {
      let child = children[i];
      if (child.tagName.toLowerCase() === "span") {
        let spanClasses = child.className.split(" ");
        let hasDbShowClass = spanClasses.some(function (className) {
          name = className.split("--")[1];
          return className.startsWith("db-show");
        });
        if (hasDbShowClass && this.#isDisplayed(child)) {
          const span_children = child.children;
          for (let i = 0; i < span_children.length; i++) {
            let span_child = span_children[i];
            if (this.#isDisplayed(span_child)) {
              if (span_child.textContent === "€") {
                currency = "EUR";
              } else if (span_child.textContent === "$") {
                currency = "USD";
              } else {
                price = span_child.textContent;
              }
            }
          }
          break;
        }
      }
    }

    const item = {
      name: name,
      currency: currency,
      quantity: 1,
      price: price,
    };

    window.dataLayer = window.dataLayer || [];
    window.dataLayer.push({
      event: "begin_checkout",
      ecommerce: {
        currency: currency,
        value: price,
        items: [item],
      },
    });

    console.log("event 'begin_checkout' sent:", {
      currency: currency,
      value: price,
      items: [item],
    });
    // alert("Redirecting to checkout page...");
  }

  #isDisplayed(element) {
    return window.getComputedStyle(element).display !== "none";
  }
}

// expected implementation when using hotwire
// export default class extends Controller {
//   static targets = ["button"];
//   static values = { currency: String, name: String, price: Number };

//   connect() {
//     console.log("Checkout controller connected");
//   }

//   sendBeginCheckoutEvent(event) {
//     const item = {
//       name: this.nameValue,
//       currency: this.currencyValue,
//       quantity: 1,
//       price: this.priceValue
//     };

//     window.dataLayer.push({
//       event: 'begin_checkout',
//       ecommerce: {
//         currency: this.currencyValue,
//         value: this.priceValue,
//         items: [item]
//       }
//     });

//     console.log("Data layer event 'begin_checkout' sent:", { currency: this.currencyValue, value: this.priceValue, items: [item] });
//   }
// }
