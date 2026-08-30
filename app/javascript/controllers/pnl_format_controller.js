import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="pnl-format"
// Switches every PnL on the page between percent and amount.
//
// The choice lives as one class on <html>, not as state inside the elements: the header total
// and each bot tile are replaced by independent turbo broadcasts, so anything toggled in place
// would snap back the moment a bot's PnL job lands. CSS in new/_utilities.sass picks the format.
export default class extends Controller {
  toggle() {
    // Nothing to switch to while balances are hidden: the amount is not in the markup at all.
    // The server also drops this controller in that state — this is the belt to that's braces.
    if (document.body.classList.contains("hide-balances")) return

    document.documentElement.classList.toggle("show-pnl-amount")
  }
}
