import { Controller } from "@hotwired/stimulus"

// Drag bot tiles around the dashboard grid; the order is saved on drop.
//
// The dragged tile is taken out of the grid entirely and a placeholder — an empty dashed cell —
// slides around in its place, marking where it will land. The tile itself is never moved while the
// drag is running: relocating the drag source under the pointer makes Chrome mis-crop its own ghost
// and mis-report which tile the pointer is over, because every insert reflows the grid the browser
// is hit-testing against. Only the placeholder moves; the real tile takes its slot on drop.
//
// ponytail: native HTML5 drag-and-drop, which never fires on touch — phone users see the saved
// order but cannot change it. Covering them means Pointer Events with a hand-rolled ghost and
// either a grip handle or long-press scroll suppression.
export default class extends Controller {
  static values = { url: String }

  connect() {
    this.savedOrder = this.ids
  }

  // dragstart is dispatched at the *source node* — the tile — so `event.target` there is useless
  // for telling where the gesture actually began. pointerdown does fire at the descendant, so the
  // origin is recorded here and read one event later.
  pointerdown(event) {
    this.fromControl = Boolean(
      event.target.closest("button, form, input, select, .dropdown-wrapper")
    )
  }

  dragstart(event) {
    const tile = event.target.closest("[data-bot-id]")

    // preventDefault is the only thing that stops a native drag; returning early just declines to
    // handle one that is already running. Started from the Start button, that would drag the tile
    // AND swallow the click the user was aiming at.
    if (!tile || this.fromControl || this.saving) {
      event.preventDefault()
      return
    }

    this.dragged = tile
    this.dropped = false
    this.orderBeforeDrag = this.ids
    // Where inside the tile it was picked up, so dragover can reconstruct where the tile now sits
    // rather than only knowing where the pointer is.
    const box = tile.getBoundingClientRect()
    this.grab = { x: event.clientX - box.left, y: event.clientY - box.top, w: box.width, h: box.height }
    event.dataTransfer.effectAllowed = "move"
    event.dataTransfer.setData("text/plain", tile.dataset.botId) // Firefox drags nothing without a payload
    event.dataTransfer.setDragImage(this.buildDragImage(tile), event.offsetX, event.offsetY)
  }

  // The ghost is supplied rather than left to the browser. Chrome's own snapshot of this tile comes
  // out as a wide, mis-cropped slab carrying fragments of its neighbours: the status bar runs a
  // marquee whose text is far wider than the tile, and the ink overflow of that plus the tile's
  // box-shadow is included in the bitmap.
  //
  // A copy with its Stimulus controllers stripped renders that text static, `overflow: hidden`
  // guarantees the bitmap stops at the tile's own edges, and it is parked exactly on top of the
  // real tile behind the page content — an element pushed off-screen to hide it snapshots blank in
  // Chrome, which is the other way this goes wrong. Chrome takes the bitmap as soon as this handler
  // returns, so the copy only has to survive one tick.
  buildDragImage(tile) {
    const box = tile.getBoundingClientRect()
    const image = tile.cloneNode(true)

    image.removeAttribute("data-controller")
    image.querySelectorAll("[data-controller]").forEach((el) => el.removeAttribute("data-controller"))
    Object.assign(image.style, {
      position: "fixed", top: `${box.top}px`, left: `${box.left}px`,
      width: `${box.width}px`, height: `${box.height}px`,
      margin: "0", overflow: "hidden", pointerEvents: "none", zIndex: "-1"
    })
    document.body.appendChild(image)
    setTimeout(() => image.remove(), 0)

    return image
  }

  // Fired at the source once the drag session is actually live, which is the one moment the ghost
  // is guaranteed to be on screen — so it is the only non-guess for pulling the tile out of the
  // grid. Doing it in dragstart hands the browser an empty tile to snapshot; doing it a frame later
  // blinks the tile out before the ghost arrives; waiting for dragover leaves both visible at once.
  drag() {
    if (!this.dragged || this.placeholder) return

    this.placeholder = this.buildPlaceholder(this.dragged)
    this.dragged.after(this.placeholder)
    this.dragged.style.display = "none" // out of the grid, so it cannot be hovered or reflow it
  }

  dragover(event) {
    if (!this.placeholder) return

    event.preventDefault() // without this the drop never fires
    event.dataTransfer.dropEffect = "move"

    // Aim with the dragged tile, not the pointer. Hit-testing `event.target` looks obvious and
    // feels wrong: the tile is 310px wide, so grabbing it near its left edge and dragging right
    // puts it almost entirely over its neighbour before the pointer has even reached that
    // neighbour's edge — and where the swap fires would change with where you happened to grab.
    //
    // The placeholder is in the running too, which is what keeps this from being twitchy: the spot
    // moves once the tile covers a neighbour more than it still covers the spot it came from.
    const over = this.mostCovered(this.draggedBox(event))
    if (!over || over === this.placeholder) return

    // Which side needs no geometry, only document order: a tile reached from before the spot takes
    // it and pushes the spot past itself, and the mirror going the other way. One rule for one
    // column or four.
    const reachedFromBefore =
      this.placeholder.compareDocumentPosition(over) & Node.DOCUMENT_POSITION_FOLLOWING
    // Element siblings, not `nextSibling`: ERB leaves whitespace text nodes between the tiles, so
    // the raw sibling is never the placeholder and this guard could never match.
    const before = reachedFromBefore ? over.nextElementSibling : over
    if (before === this.placeholder) return

    this.element.insertBefore(this.placeholder, before)
  }

  drop(event) {
    if (!this.placeholder) return

    event.preventDefault()
    this.dropped = true
    this.settle()
    this.save()
  }

  // Fires for every drag, dropped or not. A release outside the grid — or Escape — lands here with
  // nothing dropped, and the placeholder has already wandered, so the grid has to be put back.
  dragend() {
    if (!this.dropped) {
      this.settle()
      this.restore(this.orderBeforeDrag)
    }
    this.dragged = null
  }

  // The tile takes the placeholder's slot and rejoins the grid. Idempotent: drop calls it, and
  // dragend calls it again for the drags that never dropped.
  settle() {
    if (!this.placeholder) return

    this.placeholder.replaceWith(this.dragged)
    this.dragged.style.display = ""
    this.placeholder = null
  }

  // Where the dragged tile is right now: the pointer, less wherever inside the tile it was grabbed.
  draggedBox(event) {
    const left = event.clientX - this.grab.x
    const top = event.clientY - this.grab.y

    return { left, top, right: left + this.grab.w, bottom: top + this.grab.h }
  }

  // The grid cell the dragged tile covers most. The dragged tile itself is display:none and so
  // measures zero, which drops it out of the contest without a special case.
  mostCovered(box) {
    let winner = null
    let best = 0

    Array.from(this.element.children).forEach((cell) => {
      const r = cell.getBoundingClientRect()
      const w = Math.min(box.right, r.right) - Math.max(box.left, r.left)
      const h = Math.min(box.bottom, r.bottom) - Math.max(box.top, r.top)
      if (w <= 0 || h <= 0) return

      if (w * h > best) {
        best = w * h
        winner = cell
      }
    })

    return winner
  }

  // Height is copied off the tile because an empty div has none, and a collapsing cell would close
  // the very gap this is reserving.
  buildPlaceholder(tile) {
    const node = document.createElement("div")
    node.className = "bot-tile bot-tile--placeholder"
    node.style.height = `${tile.getBoundingClientRect().height}px`
    return node
  }

  save() {
    const ids = this.ids
    if (String(ids) === String(this.savedOrder)) return

    this.saving = true // dragstart refuses while this is set: one write in flight, never a queue
    fetch(this.urlValue, {
      method: "PATCH",
      headers: {
        "Content-Type": "application/json",
        "Accept": "application/json",
        "X-CSRF-Token": document.querySelector('meta[name="csrf-token"]')?.content
      },
      body: JSON.stringify({ ids: ids })
    })
      // `redirected` matters as much as `ok`: Devise answers a signed-out request with a 302 to
      // the login page, which fetch follows into a perfectly successful-looking 200.
      .then((response) => {
        if (response.ok && !response.redirected) {
          this.savedOrder = ids
        } else {
          this.restore(this.savedOrder)
        }
      })
      .catch(() => this.restore(this.savedOrder))
      .then(() => { this.saving = false })
  }

  restore(order) {
    order.forEach((id) => {
      const tile = this.element.querySelector(`[data-bot-id="${id}"]`)
      if (tile) this.element.appendChild(tile)
    })
  }

  get ids() {
    return Array.from(this.element.querySelectorAll("[data-bot-id]"), (tile) => tile.dataset.botId)
  }
}
