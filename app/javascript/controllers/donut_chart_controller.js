import { Controller } from "@hotwired/stimulus"
import { renderDonut } from "../lib/donut_chart"

// Renders a 3D donut chart with an HTML legend (color swatch + label + %).
//
// Usage:
//   <div data-controller="donut-chart" data-donut-chart-data-value='[...]'>
//     <svg data-donut-chart-target="svg"></svg>
//     <div data-donut-chart-target="legend" class="pie-legend"></div>
//   </div>
//
// Each data entry: { label, name, value, color, symbol }
//   - `symbol` and `label` default to each other when missing.
//   - `name` falls back to `label`.
export default class extends Controller {
  static targets = ["svg", "legend"]
  static values = { data: Array }

  connect() { this.render() }
  dataValueChanged() { if (this.hasSvgTarget) this.render() }

  render() {
    const data = this.dataValue || []
    if (data.length === 0) return

    renderDonut(this.svgTarget, data, { showLabels: false })
    this.renderLegend(data)
  }

  renderLegend(data) {
    const total = data.reduce((s, d) => s + d.value, 0) || 1
    this.legendTarget.innerHTML = ""

    data.forEach((item, index) => {
      const pct = ((item.value / total) * 100).toFixed(1)
      const symbol = item.symbol || item.label
      const name = item.name || item.label

      const row = document.createElement("div")
      row.className = "legend-item"
      row.dataset.index = index

      const colorInput = document.createElement("input")
      colorInput.type = "color"
      colorInput.className = "legend-color-input"
      colorInput.value = item.color
      colorInput.dataset.symbol = symbol
      colorInput.dataset.index = index
      colorInput.addEventListener("input", (e) => this.onColorChange(index, e.target.value))
      row.appendChild(colorInput)

      const label = document.createElement("span")
      label.className = "legend-label"
      label.textContent = name
      row.appendChild(label)

      const value = document.createElement("span")
      value.className = "legend-value"
      value.textContent = `${pct}%`
      row.appendChild(value)

      this.legendTarget.appendChild(row)
    })
  }

  // Live-update: recolor the matching slice + walls without re-rendering the whole chart.
  onColorChange(index, newColor) {
    const data = [...this.dataValue]
    if (!data[index]) return
    data[index] = { ...data[index], color: newColor }
    this.dataValue = data
  }
}
