import { Controller } from "@hotwired/stimulus";

// Transactions ⇄ Positions. Both panes are rendered; this only decides which one is on.
//
// A pane is `display: contents`, so its contextual filter sits up in the bar and its table below —
// while both stay inside the pane's own `order-filter` scope. Hiding it therefore has to hide its
// children, which is what the class does; `display: none` on the pane itself would do nothing.
export default class extends Controller {
  static targets = ["pane"];

  show(event) {
    this.paneTargets.forEach((pane) => {
      pane.classList.toggle("tracker-record__pane--off", pane.dataset.pane !== event.detail.value);
    });
  }
}
