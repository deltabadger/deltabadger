import { Controller } from "@hotwired/stimulus";

// Transactions ⇄ Positions. Both panes are rendered; this only decides which one is on.
//
// A pane is `display: contents`, so its contextual filter sits up in the bar and its table below —
// while both stay inside the pane's own `order-filter` scope. Hiding it therefore has to hide its
// children, which is what the class does; `display: none` on the pane itself would do nothing.
export default class extends Controller {
  static targets = ["pane"];

  // The padding is a transient hold, not page state — but Turbo snapshots the DOM BEFORE the
  // controller disconnects, so without this a restored Back page comes back carrying the blank
  // tail and none of the scroll listener that was supposed to eat it.
  connect() {
    this.uncache = () => this.#release();
    document.addEventListener("turbo:before-cache", this.uncache);
  }

  disconnect() {
    document.removeEventListener("turbo:before-cache", this.uncache);
    this.#release();
  }

  show(event) {
    this.paneTargets.forEach((pane) => {
      pane.classList.toggle("tracker-record__pane--off", pane.dataset.pane !== event.detail.value);
    });
  }

  // A filter that drops most of the rows yanks the page up under the reader: the browser clamps
  // scroll to the shorter document the moment layout is flushed, and that clamp IS the jump.
  //
  // Reading the offset is itself a layout flush, so it cannot be done after the rows are hidden —
  // it would return the already-clamped value, the slack below would come out zero, and the jump
  // this exists to stop would happen anyway. Hence two halves, wired either side of the filter.
  mark() {
    this.top = window.scrollY;
  }

  // Pad the block back to the height it had, so the table's header stays where it was on screen.
  hold() {
    const y = this.top ?? window.scrollY;
    this.#release();
    const slack = y + window.innerHeight - document.documentElement.scrollHeight;
    if (slack <= 0) return; // page grew, or never scrolled past the fold

    this.element.style.paddingBottom = `${slack}px`;
    // `behavior: instant` because the page sets `scroll-behavior: smooth` globally: an animated
    // restore would still be travelling when the listener below starts trimming, and the trim
    // would settle on the offset it was passing through rather than the one being restored.
    if (window.scrollY !== y) window.scrollTo({ top: y, left: window.scrollX, behavior: "instant" });
    window.addEventListener("scroll", this.#shrink, { passive: true });
  }

  // The void is only ever eaten, never handed back — growing it would let a reader scrolling DOWN
  // extend it forever.
  #shrink = () => {
    const pad = parseFloat(this.element.style.paddingBottom) || 0;
    const floor = document.documentElement.scrollHeight - pad;
    const needed = Math.max(0, window.scrollY + window.innerHeight - floor);
    if (needed >= pad) return;
    if (needed) this.element.style.paddingBottom = `${needed}px`;
    else this.#release();
  };

  #release() {
    this.element.style.paddingBottom = "";
    window.removeEventListener("scroll", this.#shrink);
  }
}
