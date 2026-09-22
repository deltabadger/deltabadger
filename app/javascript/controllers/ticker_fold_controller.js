import { Controller } from "@hotwired/stimulus"

// A .ticker-group--fold shows every chip, stacked in one row, as long as they fit its width; past it
// the tail folds into "+N", chip by chip until it fits. Re-folds when its chips change — the
// dashboard's merge bar re-renders them on every pick — and on resize.
export default class extends Controller {
  connect() {
    this.more = document.createElement("span")
    this.more.className = "ticker ticker--more"
    this.onResize = () => this.fold()
    this.observer = new MutationObserver(() => this.fold())
    this.observer.observe(this.element, { childList: true })
    window.addEventListener("resize", this.onResize)
    this.fold()
  }

  disconnect() {
    this.observer.disconnect()
    window.removeEventListener("resize", this.onResize)
  }

  fold() {
    const group = this.element
    const chips = [...group.children].filter(chip => chip !== this.more)
    const overflows = () => group.scrollWidth > group.clientWidth + 1
    chips.forEach(chip => { chip.hidden = false })
    this.more.remove()

    if (overflows()) {
      group.append(this.more)
      let shown = chips.length
      while (overflows() && shown > 1) {
        chips[--shown].hidden = true
        this.more.textContent = `+${chips.length - shown}`
      }
    }
    // Moving its own "+N" is not a change to fold again on.
    this.observer.takeRecords()
  }
}
