import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="order-filter"
// Filters order rows based on type (all, successful, waiting)
export default class extends Controller {
  static targets = ["row", "filter"]
  static values = { current: { type: String, default: "all" } }

  connect() {
    // Visibility is set server-side initially, only update when rows load
  }

  rowTargetConnected() {
    this.updateVisibility()
  }

  filter(event) {
    event.preventDefault()
    this.currentValue = event.currentTarget.dataset.filterType
    this.updateActiveButton()
    this.updateVisibility()
  }

  updateActiveButton() {
    this.filterTargets.forEach(button => {
      const isActive = button.dataset.filterType === this.currentValue
      button.classList.toggle("sbutton--multi--active", isActive)
    })
  }

  updateVisibility() {
    this.rowTargets.forEach(row => {
      const orderType = row.dataset.orderType
      const visible = this.currentValue === "all" || orderType === this.currentValue
      row.style.display = visible ? "" : "none"
    })
  }

}
