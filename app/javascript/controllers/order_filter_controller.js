import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="order-filter"
// Every row carries the tabs it belongs to in data-order-type: the sentence rows are
// "all" (plus "other" for cancelled/skipped/failed, which have no columnar row), the
// columnar rows are "successful" or "waiting". Filtering is a membership test, and the
// Amount/Value headers only make sense for the two columnar tabs.
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
    this.rowTargets.forEach(row => {
      // A row with no tabs at all (the columnar row of a cancelled order) shows nowhere.
      const tabs = (row.dataset.orderType || "").split(" ")
      row.style.display = tabs.includes(this.currentValue) ? "" : "none"
    })
  }

  updateHeader() {
    const sentences = this.currentValue === "all" || this.currentValue === "other"
    this.columnHeaderTargets.forEach(th => { th.style.display = sentences ? "none" : "" })
  }

}
