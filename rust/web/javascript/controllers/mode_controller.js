import { Controller } from "@hotwired/stimulus"

// D-MODE-SWITCH: the Settings menu's System/Light/Dark. `<html data-mode>` is the truth — the server writes it from the
// `mode` cookie, Turbo never replaces <html>, and a pick sets both. System is no attribute and no cookie: the OS decides.
// The row is a segmented control; this listens to its change and keeps the other menu's row (mobile or desktop) in step.
export default class extends Controller {
  // A cached Back/Forward page brings back the option chosen when it was left; `<html data-mode>` is what is in force.
  connect() {
    this.#sync(this.element, document.documentElement.dataset.mode || "system")
  }

  pick(event) {
    const value = event.detail.value
    const root = document.documentElement
    if ((root.dataset.mode || "system") === value) return
    if (value === "system") {
      delete root.dataset.mode
      document.cookie = "mode=; path=/; max-age=0; samesite=lax"
    } else {
      root.dataset.mode = value
      document.cookie = `mode=${value}; path=/; max-age=31536000; samesite=lax`
    }
    // The other copy is hidden (one menu shows per screen size), so its state is set, not clicked: a click there lands
    // outside the open dropdown and would close it. Its segmented control places the chip when it is shown.
    document.querySelectorAll("[data-mode-switch]").forEach((row) => { if (row !== this.element) this.#sync(row, value) })
  }

  #sync(row, value) {
    row.querySelectorAll("[data-value]").forEach((option) => {
      const on = option.dataset.value === value
      option.classList.toggle("is-on", on)
      option.setAttribute("aria-checked", String(on))
      option.tabIndex = on ? 0 : -1
    })
  }
}
