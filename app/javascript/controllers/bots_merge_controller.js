import { Controller } from "@hotwired/stimulus"

// The dashboard's Merge: pick bots, see their assets stack up in a bar at the bottom, confirm.
//
// A bot can join the picked ones when it spends the same currency and some venue lists every asset
// picked so far plus its own; the tiles that cannot join fade. The bar says where the merged bot will
// live when that is not the anchor's venue. A first pick no other bot can join shows no chips, only
// why: no other bot spends its currency, or none shares a venue with it. Each tile carries what this
// needs (bots/bot_tile), so no request is made until Confirm.
//
// While the mode is on, the page is locked: the only clicks that do anything are on the tiles, inside
// the bar and inside the confirmation modal. Everything else — a tile's Start/Stop, New, a filter, the
// navbar, a Turbo visit — is stopped at one capture-phase listener and shakes the bar. Turbo's own
// link and form observers run in the bubble phase and honour defaultPrevented, so one listener on
// `document` covers them all without any per-element wiring.
//
// Split uses the same mode (`enter` with mode "split"): a tile is pickable on its own — a multi-asset bot,
// data-split-ok — so there is no joining, venue or dead-end logic, one pick is enough, and the bar's
// form opens the split modal instead of the merge one.
//
// The mode is client state and nothing more: a reload, a full-body refresh or Back through the
// snapshot cache drops it, and the dashboard comes back normal. Nothing here survives a navigation.
export default class extends Controller {
  static targets = ["bar", "stack", "hint", "alone", "warning", "confirm", "ids", "form"]
  static values = {
    connected: { type: Array, default: [] },
    names: { type: Object, default: {} },
    noPartner: String,
    noSharedExchange: String,
    otherExchange: String,
    splitUrl: String,
    splitLabel: String
  }

  connect() {
    this.selected = []
    this.active = false
    this.onClick = this.click.bind(this)
    this.onPointerdown = this.stop.bind(this)
    this.onDragstart = this.block.bind(this)
    this.onVisit = this.visit.bind(this)
    this.onSubmitEnd = this.submitEnd.bind(this)
    this.onChange = this.venuePicked.bind(this)
    this.onBeforeCache = this.leave.bind(this)
    this.onShaken = () => this.barTarget.classList.remove("merge-bar--shake")
    document.addEventListener("turbo:before-cache", this.onBeforeCache)
    if (this.hasBarTarget) this.barTarget.addEventListener("animationend", this.onShaken)
  }

  disconnect() {
    this.leave()
    document.removeEventListener("turbo:before-cache", this.onBeforeCache)
    if (this.hasBarTarget) this.barTarget.removeEventListener("animationend", this.onShaken)
  }

  enter(event) {
    if (this.active || !this.hasBarTarget) return

    this.split = event?.params?.mode === "split"
    if (!this.mergeUrl) {
      this.mergeUrl = this.formTarget.action
      this.mergeLabel = this.confirmTarget.textContent
    }
    this.formTarget.action = this.split ? this.splitUrlValue : this.mergeUrl
    this.confirmTarget.textContent = this.split ? this.splitLabelValue : this.mergeLabel
    this.active = true
    this.merged = false
    this.selected = []
    this.element.classList.add("bots-merge--active")
    this.barTarget.hidden = false
    document.addEventListener("click", this.onClick, true)
    document.addEventListener("pointerdown", this.onPointerdown, true)
    document.addEventListener("dragstart", this.onDragstart, true)
    document.addEventListener("turbo:before-visit", this.onVisit)
    document.addEventListener("turbo:submit-end", this.onSubmitEnd)
    document.addEventListener("change", this.onChange)
    this.render()
  }

  cancel() {
    this.leave()
  }

  leave() {
    if (!this.active) return

    this.active = false
    document.removeEventListener("click", this.onClick, true)
    document.removeEventListener("pointerdown", this.onPointerdown, true)
    document.removeEventListener("dragstart", this.onDragstart, true)
    document.removeEventListener("turbo:before-visit", this.onVisit)
    document.removeEventListener("turbo:submit-end", this.onSubmitEnd)
    document.removeEventListener("change", this.onChange)
    this.element.classList.remove("bots-merge--active")
    this.barTarget.hidden = true
    this.selected = []
    this.tiles.forEach(tile => tile.classList.remove("bot-tile--selected", "bot-tile--faded"))
  }

  // Capture phase, before Turbo and before any Stimulus action on the target.
  click(event) {
    if (event.target.closest("#modal")) return
    if (this.barTarget.contains(event.target)) return

    event.preventDefault()
    event.stopPropagation()

    const tile = event.target.closest(".bot-tile")
    const pickable = tile && !event.target.closest(".bot-control") &&
      tile.dataset[this.okKey] === "true" && !tile.classList.contains("bot-tile--faded")
    if (pickable) {
      this.toggle(tile.dataset.botId)
      this.render()
    } else {
      this.shake()
    }
  }

  // The reorder drag listens on the grid; stopped here it never sees the gesture begin.
  stop(event) {
    if (event.target.closest("#modal") || this.barTarget.contains(event.target)) return
    event.stopPropagation()
  }

  block(event) {
    event.preventDefault()
    event.stopPropagation()
  }

  visit(event) {
    if (this.merged) return
    event.preventDefault()
    this.shake()
  }

  // Only the successful merge opens the door: its response is a stream redirect, which is a Turbo
  // visit. The confirmation GET is a frame load and never comes through here; a cancelled modal or a
  // refused POST changes nothing.
  submitEnd(event) {
    if (event.target.matches("[data-bots-merge-form]") && event.detail.success) this.merged = true
  }

  // A venue picked in the modal re-renders it; until then its Confirm still posts the previous venue.
  // Disabled for the look, and inert because Turbo re-enables a submitter whose POST was already in
  // flight. Nothing here turns it back on: only the re-rendered modal brings a live Confirm, one that
  // posts the venue it shows. A refresh that never lands leaves it dead.
  venuePicked(event) {
    if (!event.target.matches("#modal select[name=exchange_id]")) return
    const form = document.querySelector("#modal [data-bots-merge-form]")
    if (!form) return
    form.inert = true
    form.querySelectorAll("button").forEach(button => { button.disabled = true })
  }

  toggle(id) {
    const index = this.selected.indexOf(id)
    if (index === -1) this.selected.push(id)
    else this.selected.splice(index, 1)
  }

  render() {
    const tiles = this.tiles
    const picked = this.selected.map(id => tiles.find(tile => tile.dataset.botId === id)).filter(Boolean)
    const anchor = picked[0]

    tiles.forEach(tile => {
      const isPicked = picked.includes(tile)
      const joinable = tile.dataset[this.okKey] === "true" && (this.split || !anchor || this.joinable(picked, tile))
      tile.classList.toggle("bot-tile--selected", isPicked)
      tile.classList.toggle("bot-tile--faded", !isPicked && !joinable)
    })

    const alone = this.split ? null : this.alone(anchor, picked, tiles)
    this.renderStack(alone ? [] : this.members(picked))
    this.renderWarning(alone || this.split ? null : anchor, picked)
    this.hintTarget.hidden = picked.length > 0
    this.aloneTarget.textContent = alone || ""
    this.aloneTarget.hidden = !alone

    this.idsTarget.replaceChildren(...this.selected.map(id => {
      const input = document.createElement("input")
      input.type = "hidden"
      input.name = "ids[]"
      input.value = id
      return input
    }))
    this.confirmTarget.disabled = picked.length < (this.split ? 1 : 2)
  }

  get okKey() {
    return this.split ? "splitOk" : "mergeOk"
  }

  // Same currency, and one venue that lists every asset picked so far and this tile's — the anchor's
  // venue first, else a connected one, else any. A bot with orders resting on its venue can only join
  // a merge that stays on that venue: the orders are polled through it.
  joinable(picked, tile) {
    const anchor = picked[0]
    if (tile.dataset.mergeQuote !== anchor.dataset.mergeQuote) return false
    const together = [...picked, tile]
    const venue = this.venue(together)
    if (venue === null) return false
    return together.every(t => t.dataset.mergeOpenOrders !== "true" || t.dataset.mergeExchange === venue)
  }

  // The venue the merged bot would live on for these tiles, or null when none lists everything.
  venue(tiles) {
    const sets = this.members(tiles).map(chip => (chip.dataset.exchanges || "").split(",").filter(Boolean))
    const listed = sets.filter(set => set.length > 0) // an asset listed nowhere is dropped, not blocking
    if (listed.length === 0) return null
    const common = listed.reduce((acc, set) => acc.filter(id => set.includes(id)))
    if (common.length === 0) return null
    const anchorVenue = tiles[0].dataset.mergeExchange
    if (common.includes(anchorVenue)) return anchorVenue
    const connected = this.connectedValue.map(String).find(id => common.includes(id))
    return connected || common.map(Number).sort((a, b) => a - b)[0].toString()
  }

  // The union of the tiles' members, each asset once, in pick order.
  members(tiles) {
    const chips = new Map()
    tiles.forEach(tile => {
      const template = tile.querySelector("template[data-merge-chips]")
      template?.content.querySelectorAll(".ticker").forEach(chip => {
        const key = chip.dataset.tickerAssetId || chip.textContent
        if (!chips.has(key)) chips.set(key, chip)
      })
    })
    return [...chips.values()]
  }

  // Every chip, stacked; the stack folds its tail into "+N" once the row runs out of width
  // (ticker_fold_controller).
  renderStack(members) {
    const stack = this.stackTarget
    stack.replaceChildren(...members.map(chip => chip.cloneNode(true)))
    // Empty, it would still take the column's gap and push the hint off centre.
    stack.hidden = members.length === 0
    stack.classList.toggle("ticker-group--2", members.length === 2)
    stack.classList.toggle("ticker-group--many", members.length > 2)
  }

  // Why the first pick is a dead end, or null while another tile can still join it. Reads the fading
  // render() just applied.
  alone(anchor, picked, tiles) {
    if (picked.length !== 1) return null
    const others = tiles.filter(tile => tile !== anchor)
    if (others.some(tile => !tile.classList.contains("bot-tile--faded"))) return null
    const quote = anchor.dataset.mergeQuoteSymbol
    const partner = others.some(tile => tile.dataset.mergeOk === "true" &&
      tile.dataset.mergeQuote === anchor.dataset.mergeQuote)
    return (partner ? this.noSharedExchangeValue : this.noPartnerValue).replace("%{quote}", quote)
  }

  renderWarning(anchor, picked) {
    const warning = this.warningTarget
    let text = null
    if (anchor) {
      const venue = picked.length > 1 ? this.venue(picked) : anchor.dataset.mergeExchange
      if (venue && venue !== anchor.dataset.mergeExchange) {
        text = this.otherExchangeValue.replace("%{exchange}", this.namesValue[venue] || "")
      }
    }
    warning.textContent = text || ""
    warning.hidden = !text
  }

  shake() {
    const bar = this.barTarget
    bar.classList.remove("merge-bar--shake")
    void bar.offsetWidth // restart the animation even mid-shake
    bar.classList.add("merge-bar--shake")
  }

  get tiles() {
    return Array.from(this.element.querySelectorAll(".bot-tile[data-bot-id]"))
  }
}
