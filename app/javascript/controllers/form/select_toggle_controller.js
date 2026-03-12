import { Controller } from "@hotwired/stimulus"

// Shows/hides targets based on the value of a <select>.
// Each target must have data-select-toggle-value="<option_value>".
// Inputs inside hidden sections get disabled (skips validation and submission).
// Uses style.display instead of hidden attribute to avoid CSS specificity issues.
//
// Connects to data-controller="form--select-toggle"
export default class extends Controller {
  static targets = ["section"]

  connect() {
    this.toggle()
  }

  toggle() {
    const select = this.element.querySelector("select")
    const selected = select ? select.value : this.element.querySelector("input[type=hidden]")?.value
    if (!selected) return

    this.sectionTargets.forEach(el => {
      const visible = el.dataset.selectToggleValue === selected
      el.style.display = visible ? "" : "none"
      el.querySelectorAll("input, select").forEach(input => {
        if (!visible) {
          if (!input.disabled) input.dataset.disabledByToggle = ""
          input.disabled = true
        } else if (input.hasOwnProperty("dataset") && "disabledByToggle" in input.dataset) {
          input.disabled = false
          delete input.dataset.disabledByToggle
        }
      })
    })
  }
}
