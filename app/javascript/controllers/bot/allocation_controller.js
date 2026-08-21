import { Controller } from "@hotwired/stimulus";

// N weights that must sum to 100. Dragging one row gives the remainder to the other rows in
// proportion to what they already had (equal shares when they are all zero), so a user who set
// 50/25/25 and drags the first to 40 lands on 40/30/30, never 40/25/35. Only the moved range
// fires `change`, and that one submit carries every row's value.
// Connects to data-controller="bot--allocation" on the settings form.
export default class extends Controller {
  static targets = ["row", "input"];

  connect() {
    this.#paint();
  }

  update(event) {
    const moved = event.target;
    const value = Math.min(100, Math.max(0, parseFloat(moved.value) || 0));
    const others = this.inputTargets.filter(
      (input) => input !== moved && !input.disabled,
    );
    const remainder = 100 - value;
    const currentSum = others.reduce(
      (sum, input) => sum + (parseFloat(input.value) || 0),
      0,
    );

    others.forEach((input) => {
      const share =
        currentSum > 0
          ? (parseFloat(input.value) || 0) / currentSum
          : 1 / others.length;
      input.value = (remainder * share).toFixed(2);
    });
    moved.value = value.toFixed(2);
    this.#paint();
  }

  #paint() {
    this.rowTargets.forEach((row) => {
      const pct = parseFloat(row.querySelector('input[type="range"]')?.value) || 0;
      row.querySelector(
        ".slider__style__track",
      ).style.gridTemplateColumns = `${pct}% auto`;
      row.querySelector(".allocation").textContent = `${pct.toFixed(2)}%`;
    });
  }
}
