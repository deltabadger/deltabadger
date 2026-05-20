import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="order-filter"
// "all" shows the unified timeline rows (order_type "timeline"); the other tabs
// (successful/waiting/cancelled) show the columnar rows of that type. The columnar
// Amount/Value headers are hidden while the timeline is shown.
export default class extends Controller {
  static targets = ["row", "filter", "columnHeader"]
  static values = { current: { type: String, default: "all" } }

  connect() {
    this.updateHeader()
  }

  rowTargetConnected() {
    this.updateVisibility()
  }

  filter(event) {
    event.preventDefault()
    this.currentValue = event.currentTarget.dataset.filterType
    this.updateActiveButton()
    this.updateVisibility()
    this.updateHeader()
  }

  updateActiveButton() {
    this.filterTargets.forEach(button => {
      const isActive = button.dataset.filterType === this.currentValue
      button.classList.toggle("sbutton--multi--active", isActive)
    })
  }

  updateVisibility() {
    const timeline = this.currentValue === "all"
    this.rowTargets.forEach(row => {
      const orderType = row.dataset.orderType
      const visible = timeline ? orderType === "timeline" : orderType === this.currentValue
      row.style.display = visible ? "" : "none"
    })
  }

  updateHeader() {
    const timeline = this.currentValue === "all"
    this.columnHeaderTargets.forEach(th => { th.style.display = timeline ? "none" : "" })
  }

}
