import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="pnl-spark"
//
// The dashboard headline, read at a day the pointer picks. Same idea as the tracker's chart —
// hover the curve, the figure above it says what it was worth then — but with no library and no
// canvas: the plot is an SVG the server drew, and this only has to map a pointer to a point.
//
// Both formats are rewritten on every move, never just the visible one: which of them is on
// screen is a class on <html> (see pnl_format_controller.js), and the reader can flip it while
// the pointer is still on the curve.
export default class extends Controller {
  static targets = ["curve", "dot", "figure", "percent", "amount"]
  static values = {
    percent: Array,
    profit: Array,
    // The ratio the top of the box is worth — the server's, not re-derived here, or the dot would
    // sit off the curve whenever the ten-percent floor is what set the scale.
    scale: Number,
    // USD is what everything behind the page is computed in; this is the last step to the fiat the
    // account is shown in, exactly as Denomination does it server-side.
    rate: Number,
    unit: String,
    suffixed: Boolean,
    delimiter: String,
  }

  connect() {
    // Read on every connect, not once: the P/L broadcast replaces this whole element every few
    // minutes, and what it rendered is the live figure the pointer returns to.
    this.live = [
      this.percentTarget.innerHTML,
      this.hasAmountTarget ? this.amountTarget.innerHTML : null,
    ]
    this.#reserve()
    // Again once the face is in: a headline measured in the fallback font is measured in the
    // wrong one, and this figure is set at 8.2rem, where the difference is not subtle.
    document.fonts?.ready?.then(() => this.#reserve())
  }

  move(event) {
    const box = this.curveTarget.getBoundingClientRect()
    if (!box.width) return

    // A window dragged across the layout's breakpoint brings the plot back with nothing reserved.
    // Measuring here costs one layout on the first move and cannot be seen: the number has not
    // changed yet.
    if (!this.figureTarget.style.minWidth) this.#reserve()

    const last = this.percentValue.length - 1
    const at = Math.round(((event.clientX - box.left) / box.width) * last)
    this.#show(Math.min(last, Math.max(0, at)), last)
  }

  leave() {
    this.percentTarget.innerHTML = this.live[0]
    if (this.hasAmountTarget) this.amountTarget.innerHTML = this.live[1]
    this.dotTarget.hidden = true
  }

  #show(index, last) {
    const percent = this.percentValue[index]
    this.percentTarget.textContent = this.#percent(percent)
    if (this.hasAmountTarget) this.amountTarget.replaceChildren(...this.#money(this.profitValue[index]))

    // In the box's own percentages, so the dot rides the curve at any size: the viewBox is 100
    // units above the zero line and the same 100 below it.
    this.dotTarget.hidden = false
    this.dotTarget.style.left = `${(index / last) * 100}%`
    this.dotTarget.style.top = `${((1 - percent / this.scaleValue) / 2) * 100}%`
    // The ring takes the side of the day under it, not the side the curve ends on: hovering a loss
    // on a curve that ends in profit would otherwise ring a red stretch in green.
    this.dotTarget.classList.toggle("is-gain", percent >= 0)
    this.dotTarget.classList.toggle("is-loss", percent < 0)
  }

  // Hold the headline's box open at the widest reading the pointer can put in it, so the number
  // cannot resize under the pointer and drag the curve out from under it.
  //
  // MEASURED, not counted: the figures here are not tabular — "+11.72%" is three pixels narrower
  // than "+20.99%" of the same length — so no count of characters predicts the width, and CSS has
  // no way to ask. Every reading is laid out once, out of flow and invisible, in the markup that
  // will hold it; the widest wins and the ruler is thrown away.
  #reserve() {
    if (!this.hasFigureTarget) return
    // Nothing to hold open where the curve is not drawn: below the breakpoint the plot is gone,
    // and a width measured for it would be a headline wider than the phone.
    if (!this.curveTarget.getBoundingClientRect().width) {
      this.figureTarget.style.minWidth = ""
      return
    }

    const ruler = document.createElement("div")
    ruler.style.cssText = "position:absolute;visibility:hidden;white-space:nowrap"
    for (const label of new Set(this.percentValue.map((value) => this.#percent(value)))) {
      ruler.append(this.#rulerRow(label))
    }
    if (this.hasAmountTarget) {
      for (const usd of this.profitValue) ruler.append(this.#rulerRow(...this.#money(usd)))
    }

    // Inside the headline, so it is measured in the headline's own type.
    this.percentTarget.parentElement.append(ruler)
    this.figureTarget.style.minWidth = `${ruler.getBoundingClientRect().width}px`
    ruler.remove()
    // Only now is the curve drawn to the edge it will keep.
    this.element.classList.add("is-measured")
  }

  #rulerRow(...content) {
    const row = document.createElement("div")
    row.append(...content)
    return row
  }

  #percent(value) {
    return `${value > 0 ? "+" : ""}${(value * 100).toFixed(2)}%`
  }

  // The headline's amount, rebuilt the way the server writes it: sign, then the unit in <small>
  // — before the figure, or after it where the currency is written that way.
  #money(usd) {
    const value = usd * this.rateValue
    const sign = value > 0 ? "+" : value < 0 ? "-" : ""
    // Grouped by hand with the server's own mark rather than by the browser's locale, which groups
    // a Polish page with spaces where Rails wrote commas — the number would change shape the
    // moment it was hovered.
    const digits = Math.round(Math.abs(value)).toString()
      .replace(/\B(?=(\d{3})+(?!\d))/g, this.delimiterValue)
    const unit = document.createElement("small")
    unit.textContent = this.unitValue

    return this.suffixedValue ? [`${sign}${digits} `, unit] : [sign, unit, digits]
  }
}
