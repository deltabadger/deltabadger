import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="pnl-format"
// Switches every PnL on the page between percent and amount.
//
// The choice lives as one class on <html>, not as state inside the elements: the header total
// and each bot tile are replaced by independent turbo broadcasts, so anything toggled in place
// would snap back the moment a bot's PnL job lands. CSS in new/_utilities.sass picks the format.
export default class extends Controller {
  toggle() {
    document.documentElement.classList.toggle("show-pnl-amount")
  }
}
