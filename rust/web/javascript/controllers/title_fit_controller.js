import { Controller } from "@hotwired/stimulus"

// A one-line title with a shorter fallback: shows its first child while it fits the width, the second
// one otherwise. Re-checks on resize.
export default class extends Controller {
  connect() {
    this.onResize = () => this.fit()
    window.addEventListener("resize", this.onResize)
    this.fit()
  }

  disconnect() {
    window.removeEventListener("resize", this.onResize)
  }

  fit() {
    const [full, short] = this.element.children
    full.hidden = false
    short.hidden = true
    if (this.element.scrollWidth > this.element.clientWidth + 1) {
      full.hidden = true
      short.hidden = false
    }
  }
}
