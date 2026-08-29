import { Controller } from "@hotwired/stimulus";

// Lets a table's `position: sticky` header reach the top of the screen, and marks it once it has.
//
// Sticky sticks to the nearest scroll container, and a table wider than its widget makes the
// widget one (it scrolls sideways) — the header would then ride the widget, not the screen.
// Whether the table fits is the one thing CSS cannot ask, so this asks it: a widget whose
// table fits stops clipping, and native sticky does the rest.
//
// Stuck is not a state CSS can see either. A one-pixel sentinel on the widget's top edge is:
// once it has left through the top of the viewport, the header is pinned there.
export default class extends Controller {
  connect() {
    this.sentinel =
      this.element.querySelector(":scope > .table-fit__sentinel") ??
      this.element.insertAdjacentElement("afterbegin", Object.assign(document.createElement("div"), { className: "table-fit__sentinel" }));

    this.resizes = new ResizeObserver(() => this.#fit());
    this.resizes.observe(this.element);
    this.element.querySelectorAll("table").forEach((table) => this.resizes.observe(table));

    this.passing = new IntersectionObserver(([entry]) => this.#stuck(entry), { threshold: 0 });
    this.passing.observe(this.sentinel);
  }

  disconnect() {
    this.resizes.disconnect();
    this.passing.disconnect();
  }

  #fit() {
    this.element.classList.toggle("widget--table--fits", this.element.scrollWidth <= this.element.clientWidth);
  }

  // Left through the top — not merely out of view below, where the table has yet to be reached.
  #stuck(entry) {
    this.element.classList.toggle("widget--table--stuck", !entry.isIntersecting && entry.boundingClientRect.top < 0);
  }
}
