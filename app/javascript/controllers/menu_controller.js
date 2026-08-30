import { Controller } from "@hotwired/stimulus";

// The main menu's highlight: one chip that slides between the tiles, the way the .segmented thumb
// does. Turbo replaces the body on every visit, so the slide has to run on the OUTGOING page — the
// chip sets off the moment a tile is clicked and, if the new page lands mid-slide, its render is
// held until the chip has settled (turbo:before-render is cancelable and resumable). The new body
// arrives with its chip already on the tile, placed by CSS from the server's active class.
//
// Nothing here touches the DOM. The chip's real place is CSS state and the slide is a Web
// Animation: Turbo clones the outgoing body into its snapshot cache a tick after the click, and a
// class or an inline style flipped here would be cloned with it — Back would then restore the bots
// page with the tracker tile lit. An Animation is not part of the clone.
//
// Known: a second tile clicked while the first response is being held cancels that slide, which
// resumes the held render (Turbo has no cancellation past that point), so the first page shows
// until the second response lands and its own controller slides on from there — what a slightly
// slower hand would have seen. Non-destructive; not worth a render-skipping hack, which would
// leave the live page with clones of its turbo-permanent elements.
export default class extends Controller {
  static targets = ["chip", "tile"];

  connect() {
    document.addEventListener("turbo:before-render", this.beforeRender);
  }

  disconnect() {
    document.removeEventListener("turbo:before-render", this.beforeRender);
  }

  // No preventDefault: the link navigates. A MODIFIED click opens a new tab and leaves this page
  // as it was, so the chip stays put — the same rule the segmented control has.
  select(event) {
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button > 0) return;
    this.#slide(event.currentTarget);
  }

  // Back/forward and redirects carry no click: the incoming body says which tile is active and
  // the chip slides there before the swap. Then the hold, for a slide from either path.
  beforeRender = (event) => {
    const key = event.detail.newBody.querySelector(".menu__section--main-menu .menu__item--active")?.dataset.tile;
    const tile = this.tileTargets.find((candidate) => candidate.dataset.tile === key);
    if (tile) this.#slide(tile);
    // A hidden document gets no rendering opportunities, so a running animation never reports
    // finished there — a visit a broadcast started in a background tab would be held until the tab
    // was looked at. Nobody sees the slide either way: swap, and let CSS place the chip.
    if (document.hidden || this.animation?.playState !== "running") return;

    event.preventDefault();
    this.animation.finished.then(event.detail.resume, event.detail.resume);
  };

  #slide(tile) {
    // The swap places it; nothing to slide. The reset zeroes CSS transitions under reduced
    // motion, but a Web Animation has to be told.
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    // Chip or whole menu display: none — the mobile breakpoint, or a page with no tile of its own.
    if (!this.chipTarget.offsetParent) return;
    // The response landed mid-slide, on its way to this very tile: restarting would hitch the easing.
    if (this.animation?.playState === "running" && this.tile === tile) return;

    const visual = this.chipTarget.getBoundingClientRect().top; // where the eye has it, mid-slide or not
    this.animation?.cancel();
    const base = this.chipTarget.getBoundingClientRect().top; // where CSS has it
    const target = tile.getBoundingClientRect().top;
    if (visual === target) return;

    this.tile = tile;
    this.animation = this.chipTarget.animate(
      [{ transform: `translateY(${visual - base}px)` }, { transform: `translateY(${target - base}px)` }],
      { duration: 180, easing: "cubic-bezier(0.22, 0.61, 0.36, 1)", fill: "forwards" }
    );
  }
}
