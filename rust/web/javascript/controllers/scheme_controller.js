import { Controller } from "@hotwired/stimulus"

// D-SCHEME-SWITCH: the Settings menu's colour scheme circles. `<html data-scheme>` is the truth — the server writes it
// from the `scheme` cookie, Turbo never replaces <html>, and a pick sets both. Nothing is posted.
//
// Each row is put back on the scheme in force when it connects (a cached Back/Forward page brings back the circle the
// reader left), and every other row (the mobile menu's, the desktop dropdown's) follows a pick.
export default class extends Controller {
  static targets = ["option"]

  connect() {
    this.#sync(this.element)
  }

  pick(event) {
    const value = event.currentTarget.dataset.value
    const root = document.documentElement
    if (value === "default") {
      delete root.dataset.scheme
      document.cookie = "scheme=; path=/; max-age=0; samesite=lax"
    } else {
      root.dataset.scheme = value
      document.cookie = `scheme=${value}; path=/; max-age=31536000; samesite=lax`
    }
    document.querySelectorAll("[data-scheme-switch]").forEach((row) => this.#sync(row))
  }

  // A radio group's keys: the arrows move to the next circle and pick it.
  keys(event) {
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key]
    if (!step) return
    event.preventDefault()
    const options = this.optionTargets
    const next = options[(options.indexOf(event.currentTarget) + step + options.length) % options.length]
    next.focus()
    next.click()
  }

  #sync(row) {
    const current = document.documentElement.dataset.scheme || "default"
    row.querySelectorAll("[data-value]").forEach((option) => {
      const on = option.dataset.value === current
      option.classList.toggle("is-on", on)
      option.setAttribute("aria-checked", String(on))
      option.tabIndex = on ? 0 : -1
    })
  }
}
