import { Controller } from "@hotwired/stimulus"
import { createInvestAnimation } from "../lib/pixel_invest"

// The empty bots page's pixel garden (lib/pixel_invest.js), drawn in the colours of the scheme in force: the page's
// own background for the depth fade, its greys for the ground and the plants. A scheme picked in the menu, or the OS
// turning light or dark, recolours it in place. Not drawn for a reader who asked for less motion; stopped when Turbo
// takes the page away.
const VARS = { bg: "--paper", groundTop: "--steel", dust: "--concrete", stem: "--concrete", leaf: "--stone", leafL: "--ash", deadStem: "--zinc", deadLeaf: "--steel" }

// The library mixes colours as #rrggbb; a variable in any other form keeps the library's own colour.
function themed() {
  const style = getComputedStyle(document.documentElement)
  const colors = {}
  for (const [key, name] of Object.entries(VARS)) {
    const value = style.getPropertyValue(name).trim()
    if (/^#[0-9a-f]{6}$/i.test(value)) colors[key] = value
  }
  return colors
}

export default class extends Controller {
  connect() {
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) return
    this.animation = createInvestAnimation(this.element, { scale: 4, palette: "mono", coins: "stocks", colors: themed() })
    this.recolor = () => this.animation.recolor(themed())
    this.schemes = new MutationObserver(this.recolor)
    this.schemes.observe(document.documentElement, { attributes: true, attributeFilter: ["data-scheme"] })
    this.dark = matchMedia("(prefers-color-scheme: dark)")
    this.dark.addEventListener("change", this.recolor)
  }

  disconnect() {
    this.animation?.destroy()
    this.schemes?.disconnect()
    this.dark?.removeEventListener("change", this.recolor)
  }
}
