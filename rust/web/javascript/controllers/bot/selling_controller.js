import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="bot--selling" on the bot page's columns wrapper.
// The state element (`selling_state_<bot>`) is broadcast-replaced whenever the bot's selling state
// changes, and the holdings panel is replaced by figure publications; both reconnect their targets,
// so the state is applied again from the element itself every time. While anything blocks a sale,
// every Sell link is inert; the links of assets being sold also wear the spinner (`.table__busy`,
// the button kept as an invisible ghost so the column keeps its width). The door refusal on the
// server stays the backstop for anything this misses.
export default class extends Controller {
  static targets = ["state", "sell"]

  connect() { this.apply() }
  stateTargetConnected() { this.apply() }
  sellTargetConnected() { this.apply() }

  // Enter on a focused link fires a click too, so this one listener covers the keyboard.
  // Turbo skips a click whose default was prevented.
  guard(event) {
    if (event.target.closest?.('[data-bot--selling-target="sell"][aria-disabled="true"]')) {
      event.preventDefault()
      event.stopPropagation()
    }
  }

  apply() {
    if (!this.listening) {
      this.element.addEventListener("click", (e) => this.guard(e), true)
      this.listening = true
    }
    const state = this.hasStateTarget ? this.stateTarget : null
    const blocked = state?.dataset.blocked === "true"
    let assets = []
    try { assets = JSON.parse(state?.dataset.assets || "[]").map(String) } catch { assets = [] }
    this.sellTargets.forEach((link) => {
      const busy = blocked && link.dataset.asset !== undefined && assets.includes(link.dataset.asset)
      if (blocked) {
        link.setAttribute("aria-disabled", "true")
        link.setAttribute("tabindex", "-1")
      } else {
        link.removeAttribute("aria-disabled")
        link.removeAttribute("tabindex")
      }
      this.busy(link, busy)
    })
  }

  busy(link, on) {
    const wrapper = link.parentElement?.classList.contains("table__busy") ? link.parentElement : null
    if (on && !wrapper) {
      const span = document.createElement("span")
      span.className = "table__busy"
      const spinner = document.createElement("span")
      spinner.className = "loader--small"
      spinner.setAttribute("role", "status")
      spinner.setAttribute("aria-label", this.hasStateTarget ? this.stateTarget.dataset.label || "" : "")
      link.replaceWith(span)
      span.append(link, spinner)
    } else if (!on && wrapper) {
      wrapper.replaceWith(link)
    }
  }
}
