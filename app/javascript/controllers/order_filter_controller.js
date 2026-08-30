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
  // and comes back rendered on its default, while this controller — which lives on an ancestor and
  // survives — is still filtering. Re-select what the user actually chose; if that tab no longer
  // exists (its last order changed category), fall back to the default the server just rendered.
  //
  // The fallback comes off the CONTAINER, not from a constant and not from this element: the
  // container is what the broadcast replaces, so its default is as fresh as the tabs in it, while
  // this element survives the whole page and anything parked on it would be page-load stale. The
  // set of tabs is fluid — "All" is absent while balances are hidden, and a category disappears
  // when its last order changes kind — so a hard-coded "all" could name a tab that is not there
  // and hide every row.
  filterContainerTargetConnected(container) {
    const fallback = container.dataset.orderFilterDefault || "all"
    if (this.currentValue === fallback) return

    const option = container.querySelector(`[data-value="${this.currentValue}"]`)
    if (option) return option.click()

    this.currentValue = fallback
    this.updateVisibility()
    this.updateHeader()
  }

  rowTargetConnected() {
    this.updateVisibility()
  }

  // Fired by the shared `segmented` control, which owns the chip and the pressed state. This
  // only decides what the list shows.
  filter(event) {
    this.lockColumns()
    this.currentValue = event.detail.value
    this.updateVisibility()
    this.updateHeader()
  }

  // A shrink-to-fit table measures its columns from the rows that are VISIBLE, so every tab used to
  // move every column. Pin the widths the unfiltered table asks for, the first time it is filtered:
  // by then it is on screen with its fonts settled, and still showing everything it has. A table
  // that swaps its own headers (columnHeader) has no one set of columns to pin, so it keeps
  // measuring itself.
  lockColumns() {
    if (this.locked || this.hasColumnHeaderTarget) return
    this.locked = true

    const table = this.element.querySelector("table")
    const headers = table?.tHead?.rows[0]?.cells
    if (!headers) return

    for (const th of headers) th.style.width = `${th.getBoundingClientRect().width}px`
    table.style.tableLayout = "fixed"
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
