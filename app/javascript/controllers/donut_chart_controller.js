import { Controller } from "@hotwired/stimulus"
import { renderDonut } from "../lib/donut_chart"

// Renders the portfolio as a 3D donut (in-SVG labels + leader lines + logos, with
// `.ticker` hover tooltips), and toggles between that donut and a list view on click
// — the chosen view is remembered in localStorage. Mirrors the index-bot allocation
// donut (see index_allocation_controller.js).
//
// Usage:
//   <div data-controller="donut-chart"
//        data-donut-chart-data-value='[...]'
//        data-donut-chart-storage-key-value="tracker-portfolio-view"
//        data-donut-chart-other-label-value="Other">
//     <div data-donut-chart-target="pie"  data-action="click->donut-chart#toggle">
//       <svg data-donut-chart-target="svg"></svg>
//     </div>
//     <div data-donut-chart-target="list" data-action="click->donut-chart#toggle">…</div>
//   </div>
//
// Each data entry: { label, name, value, color, symbol, assetId, logo }
//   - `symbol` and `label` default to each other when missing; `assetId`/`logo` optional.
export default class extends Controller {
  static targets = ["svg", "pie", "list"]
  static values = { data: Array, storageKey: String, otherLabel: String }

  connect() {
    if (this.hasPieTarget && this.hasListTarget) this.applyView(this.#storedView())
    else this.renderPie()
  }

  // Re-render the live donut on in-place value updates (e.g. Turbo morph), but only
  // when it's the active view. `this.view` is undefined until connect, so this is a
  // no-op during Stimulus' initial value callback (connect handles the first render).
  dataValueChanged() {
    if (this.view === "pie") this.renderPie()
  }

  // Clicking either visualization flips to the other (pie default, remembered).
  toggle(event) {
    event?.preventDefault()
    const next = this.view === "pie" ? "list" : "pie"
    this.applyView(next)
    this.#persistView(next)
  }

  applyView(view) {
    this.view = view === "list" ? "list" : "pie"
    const pie = this.view === "pie"

    if (this.hasPieTarget) this.pieTarget.classList.toggle("is-hidden", !pie)
    if (this.hasListTarget) this.listTarget.classList.toggle("is-hidden", pie)

    if (pie) this.renderPie()
  }

  renderPie() {
    const data = this.dataValue || []
    if (!this.hasSvgTarget || data.length === 0) return

    // labelColor: currentColor so slice names follow the theme text color. `%`/leaders
    // stay muted grey via CSS. Small slices below 4% collapse into one "Other" legend
    // entry, but every slice is still drawn and hoverable.
    renderDonut(this.svgTarget, data, {
      showLabels: true,
      tooltips: true,
      labelColor: "currentColor",
      otherThreshold: 0.04,
      otherLabel: this.hasOtherLabelValue ? this.otherLabelValue : "Other"
    })
  }

  #storedView() {
    if (!this.storageKeyValue) return "pie"
    try {
      return localStorage.getItem(this.storageKeyValue) || "pie"
    } catch {
      return "pie"
    }
  }

  #persistView(view) {
    if (!this.storageKeyValue) return
    try {
      localStorage.setItem(this.storageKeyValue, view)
    } catch {
      // storage unavailable/blocked — preference simply isn't remembered
    }
  }
}
