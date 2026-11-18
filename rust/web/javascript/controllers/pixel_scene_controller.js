import { Controller } from "@hotwired/stimulus"
import { createInvestAnimation } from "../lib/pixel/invest"
import { createTrackAnimation } from "../lib/pixel/track"
import { createWithdrawAnimation } from "../lib/pixel/withdraw"

// D-PIXEL-SCENES: the empty states' pixel scenes (lib/pixel/*.js) — invest on the bots page, track on the tracker,
// withdraw on the rules page. Everything that is scenery (ground, plants, tower, chest, robot) is drawn in the scheme's
// own colours, and so is the astronaut's suit on a light page; the coins and the lights keep theirs. A scheme picked in
// the menu, or the OS turning light or dark, recolours the scene in place. Not drawn for a reader who asked for less
// motion. When Turbo takes the page away the scene stops (the garden only pauses: see `kept`).
//
// A colour is a variable, or [a, b, t]: t of the way from variable a to variable b.
const PLANTS = { bg: "--paper", stem: "--concrete", leaf: "--stone", leafL: "--ash" }
const SCENES = {
  invest: {
    create: createInvestAnimation,
    keep: true,
    colors: { ...PLANTS, groundTop: "--steel", dust: "--concrete", deadStem: "--zinc", deadLeaf: "--steel" },
  },
  track: {
    create: createTrackAnimation,
    colors: {
      ...PLANTS, deck: "--concrete", deckDark: "--steel", frame: ["--steel", "--concrete", 0.5], rail: "--steel",
      metal: "--ash", metalDark: "--concrete", screen: "--silver", bezel: "--steel",
    },
  },
  withdraw: {
    create: createWithdrawAnimation,
    colors: {
      bg: "--paper", groundTop: "--steel",
      chestO: "--steel", chestW: ["--steel", "--concrete", 0.5], chestH: "--stone", rivet: "--ash", panel: "--zinc",
      stripe: "--stone", meterOff: "--steel", inside: "--silver",
      metal: "--ash", metalDark: "--concrete", dog: "--stone", dogLight: "--ash", dogDark: "--concrete",
    },
  },
}

// D-PIXEL-SCENES: the bots page's garden is kept while the app runs. Leaving the page pauses it; coming back carries it
// on to the new canvas where it was. Only closing the app (or the browser tab, or a reload) plants a new one.
const kept = {}

const HEX = /^#[0-9a-f]{6}$/i
const rgb = (c) => [1, 3, 5].map((i) => parseInt(c.slice(i, i + 2), 16))
const mix = (a, b, t) => "#" + rgb(a).map((v, i) => Math.round(v + (rgb(b)[i] - v) * t).toString(16).padStart(2, "0")).join("")

// The astronaut: the libraries' white suit on a dark page; on a light one the scheme's mid greys, darker than the page.
const SUIT_DARK = { suit: "#e6edf9", suitShade: "#aebbd4", pack: "#8a9bbd", boot: "#6c7fa8" }
const SUIT_LIGHT = { suit: "--concrete", suitShade: "--stone", pack: "--ash", boot: "--ash" }
const light = (hex) => { const [r, g, b] = rgb(hex); return 0.2126 * r + 0.7152 * g + 0.0722 * b > 128 }

// The libraries mix colours as #rrggbb; a variable in any other form keeps the library's own colour. A source starting
// with # is a colour as it is.
function themed(scene) {
  const style = getComputedStyle(document.documentElement)
  const read = (name) => (name.startsWith("#") ? name : style.getPropertyValue(name).trim())
  const paper = read("--paper")
  const spec = { ...scene, ...(HEX.test(paper) && light(paper) ? SUIT_LIGHT : SUIT_DARK) }
  const colors = {}
  for (const [key, source] of Object.entries(spec)) {
    const [a, b] = (Array.isArray(source) ? source.slice(0, 2) : [source]).map(read)
    if (Array.isArray(source) ? HEX.test(a) && HEX.test(b) : HEX.test(a)) colors[key] = Array.isArray(source) ? mix(a, b, source[2]) : a
  }
  return colors
}

export default class extends Controller {
  static values = { name: String }

  connect() {
    const scene = SCENES[this.nameValue]
    if (!scene || matchMedia("(prefers-reduced-motion: reduce)").matches) return
    if (scene.keep && kept[this.nameValue]) {
      this.animation = kept[this.nameValue]
      this.animation.attach(this.element)
      this.animation.recolor(themed(scene.colors))
    } else {
      this.animation = scene.create(this.element, { scale: 4, palette: "mono", coins: "stocks", colors: themed(scene.colors) })
      if (scene.keep) kept[this.nameValue] = this.animation
    }
    this.recolor = () => this.animation.recolor(themed(scene.colors))
    this.schemes = new MutationObserver(this.recolor)
    this.schemes.observe(document.documentElement, { attributes: true, attributeFilter: ["data-scheme"] })
    this.dark = matchMedia("(prefers-color-scheme: dark)")
    this.dark.addEventListener("change", this.recolor)
  }

  disconnect() {
    if (SCENES[this.nameValue]?.keep) this.animation?.pause()
    else this.animation?.destroy()
    this.schemes?.disconnect()
    this.dark?.removeEventListener("change", this.recolor)
  }
}
