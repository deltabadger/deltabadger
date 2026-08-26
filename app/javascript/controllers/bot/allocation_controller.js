import { Controller } from "@hotwired/stimulus";

// Each slider is independent: the visible total lets the user compose weights one by one, and the
// server-side Normalize action is the deliberate way to squeeze their proportions to 100%.
// Connects to data-controller="bot--allocation" on the settings form.
export default class extends Controller {
  static targets = ["row", "input", "percent", "total", "totalRow"];

  connect() {
    this.rowTargets.forEach((row) => this.#paintRow(row));
    this.#paintTotal();
  }

  // Slider dragged: the number field follows it.
  update(event) {
    this.#paintRow(this.#rowOf(event.target));
    this.#paintTotal();
  }

  // Number typed: the slider follows it, and the field's own text is left alone mid-keystroke.
  type(event) {
    const row = this.#rowOf(event.target);
    if (!row) return;

    const range = row.querySelector('input[type="range"]');
    range.value = Math.min(100, Math.max(0, parseFloat(event.target.value) || 0));
    this.#paintTrack(row, parseFloat(range.value) || 0);
    this.#paintTotal();
  }

  #rowOf(element) {
    return element.closest('[data-bot--allocation-target="row"]');
  }

  #paintRow(row) {
    if (!row) return;

    const pct = parseFloat(row.querySelector('input[type="range"]')?.value) || 0;
    this.#paintTrack(row, pct);
    row.querySelector(".allocation__input").value = pct.toFixed(1);
  }

  #paintTrack(row, pct) {
    row.querySelector(".slider__style__track").style.gridTemplateColumns =
      `${pct}% auto`;
  }

  #paintTotal() {
    // Disabled inputs still describe a locked/running portfolio and belong in its displayed total.
    const sum = this.inputTargets.reduce(
      (total, input) => total + (parseFloat(input.value) || 0),
      0,
    );
    this.totalTarget.textContent = `${sum.toFixed(1)}%`;
    const unbalanced = Math.abs(sum - 100) > 0.1;
    this.totalRowTarget.classList.toggle(
      "asset-allocations__total--off",
      unbalanced,
    );
    [
      this.element.querySelector(".asset-allocations__normalize"),
      this.element.querySelector(".asset-allocations__hint"),
    ].forEach((element) => {
      if (element) element.hidden = !unbalanced;
    });
  }
}
