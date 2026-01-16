import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="order-filter"
// Filters order rows based on type (all, successful, waiting)
export default class extends Controller {
  static targets = ["row", "filter", "filterContainer"]
  static values = { current: { type: String, default: "all" } }

  connect() {
    // Visibility is set server-side initially, only update when rows load
  }

  rowTargetConnected() {
    this.updateFilterVisibility()
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
      button.classList.toggle("button--multi--active", isActive)
    })
  }

  updateVisibility() {
    this.rowTargets.forEach(row => {
      const orderType = row.dataset.orderType
      const visible = this.currentValue === "all" || orderType === this.currentValue
      row.style.display = visible ? "" : "none"
    })
  }

  updateFilterVisibility() {
    if (!this.hasFilterContainerTarget) return

    let hasSuccessful = false
    let hasWaiting = false
    let hasOther = false // skipped, failed, etc.

    this.rowTargets.forEach(row => {
      const orderType = row.dataset.orderType
      if (orderType === "successful") hasSuccessful = true
      else if (orderType === "waiting") hasWaiting = true
      else hasOther = true
    })

    // Show filters if there's more than one category of orders
    const categories = [hasSuccessful, hasWaiting, hasOther].filter(Boolean).length
    const shouldShow = categories > 1 || (hasSuccessful && hasWaiting)
    this.filterContainerTarget.style.display = shouldShow ? "" : "none"
  }
}
