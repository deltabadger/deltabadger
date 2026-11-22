import { Controller } from "@hotwired/stimulus"
import { stackLayout } from "../lib/toast_stack.js"

// Connects to data-controller="flash-stack" (layouts/application.html, layouts/devise.html): every visible .salert in the
// stack is one toast, wherever in the stack it sits (#flash, #sync-warnings, #desktop_update) and however it came (the
// page, a stream's append, prepend or replace). Each gets a glyph whose ring is its timer (drained, it leaves), a progress
// fill (the tax report's bar) or a spinner (a sync under way); a message with a button (or the update banner with its link)
// has no timer, and one waiting behind a timed one holds its own until it is in front. Newest in
// front, older ones tucked behind; hover or focus fans them out and holds the timers (the CSS). Dragging one right past
// 80px, or Escape inside it (each is a tab stop), sends it away: one a stream can still answer by its id (the tax report's
// progress, the update banner) is hidden rather than removed, so the answer still lands. Styles: new/_salert.sass.

// Arrival order, across visits: #flash is turbo-permanent, so its toasts keep their numbers when this stack reconnects.
let seq = 0
const GLYPH = '<svg class="toast__glyph" viewBox="0 0 24 24" aria-hidden="true"><circle class="toast__track" cx="12" cy="12" r="9"/>' +
  '<circle class="toast__ring" cx="12" cy="12" r="9"/><circle class="toast__dot" cx="12" cy="12" r="4.5"/></svg>'
const EVENTS = ["mouseenter", "mouseleave", "focusin", "focusout", "animationend", "keydown", "mousedown", "pointerdown", "pointermove", "pointerup", "pointercancel",
  "turbo:before-morph-element"]
const BUSY = ".salert--syncing, .salert--download"

export default class extends Controller {
  connect() {
    // A page's own stack (the tracker's pending tax report) joins this one: one stack on screen.
    for (const other of document.querySelectorAll(".flash-stack")) {
      if (!this.element.contains(other)) this.element.append(...other.childNodes)
    }
    this.observer = new MutationObserver((records) => this.refresh(records))
    this.observer.observe(this.element, { childList: true, subtree: true, attributes: true, attributeFilter: ["class"] })
    EVENTS.forEach((type) => this.element.addEventListener(type, this))
    addEventListener("resize", this.layout)
    this.refresh([])
  }

  disconnect() {
    this.observer.disconnect()
    EVENTS.forEach((type) => this.element.removeEventListener(type, this))
    removeEventListener("resize", this.layout)
  }

  toasts() {
    return [...this.element.querySelectorAll(".salert")].filter((t) => !t.classList.contains("hidden") && t.dataset.state !== "out")
  }

  refresh(records) {
    // A message a stream replaced by id (the tax report's progress, and how it ends) keeps its place and its ring.
    const replaced = new Map()
    for (const record of records) {
      for (const node of record.removedNodes) if (node.id && (node.dataset?.seq || "dismissed" in (node.dataset || {}))) replaced.set(node.id, node)
    }
    // Sent away while still at work, and replaced by more of the same (the next percent): still away. How it ends shows.
    for (const t of this.element.querySelectorAll(".salert[id]:not(.hidden)")) {
      if ("dismissed" in (replaced.get(t.id)?.dataset || {}) && t.matches(BUSY)) { t.dataset.dismissed = ""; t.classList.add("hidden") }
    }
    for (const hidden of this.element.querySelectorAll(".salert.hidden")) delete hidden.dataset.seq
    for (const t of this.toasts()) {
      if (t.dataset.seq) { this.decorate(t); continue }
      const old = replaced.get(t.id)
      if (old?.dataset.seq) {
        t.dataset.seq = old.dataset.seq
        this.decorate(t, old.querySelector(".toast__glyph")?.style.getPropertyValue("--p"))
      } else {
        t.dataset.seq = ++seq
        t.dataset.state = "in"
        requestAnimationFrame(() => requestAnimationFrame(() => delete t.dataset.state))
        this.decorate(t)
      }
    }
    this.layout()
  }

  // The glyph beside the message's own markup; the ring's job read from what the message carries.
  decorate(t, from) {
    let body = t.querySelector(":scope > .toast__body")
    const track = t.querySelector(".slider__style__track")
    if (!body) {
      body = document.createElement("div")
      body.className = "toast__body"
      body.innerHTML = GLYPH + '<div class="toast__text"></div>'
      body.lastChild.append(...t.childNodes)
      t.append(body)
      if (track) {
        const glyph = body.firstChild, p = (parseFloat(track.style.gridTemplateColumns) || 0) / 100
        glyph.style.setProperty("--p", from || p)
        if (from) requestAnimationFrame(() => requestAnimationFrame(() => glyph.style.setProperty("--p", p)))
      }
    }
    const glyph = body.firstChild
    // A button, or a link drawn as one (a key's repair), is a question: no timer. A link in the text is not.
    const ring = t.querySelector(t.id === "desktop_update" ? "a:not(.hidden), button:not(.hidden)" : "button:not(.hidden), a.rbutton:not(.hidden)") ? "still"
      : track ? "pct" : t.matches(BUSY) ? "spin" : "timer"
    if (glyph.dataset.ring !== ring) glyph.dataset.ring = ring
    // A message that changes kind in place (the update banner turning critical) lands again: an error shakes.
    const kind = ["success", "warning", "danger"].find((k) => t.classList.contains(`salert--${k}`)) || "notice"
    if (t.dataset.kind && t.dataset.kind !== kind) delete t.dataset.settled
    t.dataset.kind = kind
    if (!t.hasAttribute("tabindex")) t.tabIndex = 0
  }

  // Newest in front. Collapsed, the ones behind take the front one's height and peek out above it.
  layout = () => {
    const live = this.toasts().sort((a, b) => b.dataset.seq - a.dataset.seq)
    const open = this.element.matches(":hover, :focus-within")
    const heights = live.map((t) => (t.querySelector(":scope > .toast__body")?.offsetHeight ?? 0) + 2)
    const { items, height } = stackLayout(heights, open)
    const ring = (t) => t.querySelector(".toast__glyph")?.dataset.ring
    live.forEach((t, i) => {
      t.toggleAttribute("data-back", items[i].back)
      // Unreadable behind a message that will leave by itself: its own time starts when it can be read.
      t.toggleAttribute("data-waiting", items[i].back && ring(live[0]) === "timer")
      t.style.height = items[i].height + "px"
      t.style.setProperty("--y", items[i].y + "px")
      t.style.setProperty("--s", items[i].scale)
      t.style.setProperty("--o", items[i].opacity)
      t.style.zIndex = 100 - i
    })
    this.element.style.height = height + "px"
  }

  dismiss(t, dx) {
    if (t.dataset.state === "out") return
    t.dataset.state = "out"
    t.style.setProperty(dx ? "--dx" : "--y", dx ? "120%" : `calc(${t.style.getPropertyValue("--y") || "0px"} + 12px)`)
    if (t.contains(document.activeElement)) document.activeElement.blur()
    // Still at work under an id: a stream will answer it, so it is hidden, not removed.
    const kept = t.id && t.matches(BUSY)
    if (kept) { t.dataset.dismissed = ""; t.dispatchEvent(new CustomEvent("toast:dismissed", { bubbles: true })) }
    setTimeout(() => {
      if (!kept) return t.remove()
      delete t.dataset.state
      t.style.removeProperty("--dx")
      // Its state moved on meanwhile (tauri.js): it stays, laid out again.
      "dismissed" in t.dataset ? t.classList.add("hidden") : this.layout()
    }, 260)
    this.layout()
  }

  handleEvent(e) {
    const t = e.target.closest?.(".salert")
    switch (e.type) {
      case "focusout": return setTimeout(this.layout)
      case "animationend":
        if (!t) return
        if (e.animationName === "toast-drain") return this.dismiss(t)
        if (e.animationName === "toast-shake" || e.animationName === "toast-hop") t.dataset.settled = ""
        return
      case "keydown": if (e.key === "Escape" && t) this.dismiss(t); return
      // A cached page's quiet revisit (quiet_revisit.js) morphs the body: the stack stays as it stands, its messages are
      // the live ones (#flash and #sync-warnings are permanent, the update banner is tauri.js's).
      case "turbo:before-morph-element": if (e.target === this.element) e.preventDefault(); return
      // A press is a drag, not a focus: the stack would stay fanned out, and its timers held, until a click elsewhere.
      case "mousedown": if (t && !e.target.closest("a, button, input")) e.preventDefault(); return
      // Drag right to throw away.
      case "pointerdown":
        if (!t || e.target.closest("a, button, input")) return
        this.drag = { t, x: e.clientX, dx: 0 }
        t.setPointerCapture(e.pointerId)
        return
      case "pointermove": {
        if (!this.drag) return
        const dx = Math.max(0, e.clientX - this.drag.x)
        this.drag.t.toggleAttribute("data-dragging", dx > 0)
        this.drag.t.style.setProperty("--dx", dx + "px")
        this.drag.dx = dx
        return
      }
      case "pointerup": case "pointercancel":
        if (!this.drag) return
        this.drag.t.removeAttribute("data-dragging")
        this.drag.dx > 80 ? this.dismiss(this.drag.t, true) : this.drag.t.style.setProperty("--dx", "0px")
        this.drag = null
        return
      default: this.layout() // mouseenter, mouseleave, focusin
    }
  }
}
