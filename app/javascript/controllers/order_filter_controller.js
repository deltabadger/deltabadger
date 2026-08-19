import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="order-filter"
// "all" shows the unified timeline rows (order_type "timeline"); the other tabs
// (successful/waiting/cancelled) show the columnar rows of that type. The columnar
// Amount/Value headers are hidden while the timeline is shown.
export default class extends Controller {
  static targets = ["row", "columnHeader", "filterContainer"]
  static values = { current: { type: String, default: "all" } }

  connect() {
    this.updateHeader()
  }

  // The filter row is broadcast-replaced whenever an order changes (Bot#broadcast_order_filters_update)
  // and comes back rendered on "all", while this controller — which lives on an ancestor and
  // survives — is still filtering. Re-select what the user actually chose; if that tab no longer
  // exists (its last order changed category), fall back to the "all" the server just rendered.
  filterContainerTargetConnected(container) {
    if (this.currentValue === "all") return

    const option = container.querySelector(`[data-value="${this.currentValue}"]`)
    if (option) return option.click()

    this.currentValue = "all"
    this.updateVisibility()
    this.updateHeader()
  }

  rowTargetConnected() {
    this.updateVisibility()
  }

  // Fired by the shared `segmented` control, which owns the chip and the pressed state. This
  // only decides what the list shows.
  filter(event) {
    this.currentValue = event.detail.value
    this.updateVisibility()
    this.updateHeader()
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
