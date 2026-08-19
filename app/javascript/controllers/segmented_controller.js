import { Controller } from "@hotwired/stimulus";

// The segmented control: moves the chip to the selected option and tells whoever is listening.
//
// It owns the CHIP and the SELECTED state, nothing else. Consumers listen for the
// `segmented:change` event it dispatches and act on `detail.value` — the chart redraws, the
// order list re-filters, and a link-based group just navigates, because clicks are never
// intercepted here.
//
// One measurement path for both sizings: the chip takes the selected option's own left and
// width, which is what makes the FLUID variant work and costs the EQUAL one nothing.
export default class extends Controller {
  static targets = ["thumb", "option"];

  connect() {
    // The chip is measured in pixels, so anything that reflows a label has to re-measure it.
    // A ResizeObserver on the options catches every cause at once — the container resizing, a
    // longer label in another locale, and above all the webfont swap: `document.fonts.ready`
    // resolves before the relayout, which leaves a chip sized to the fallback face sitting
    // over a narrower option. It also fires once on observe, which is the first placement.
    this.observer = new ResizeObserver(() => this.#moveThumb());
    this.optionTargets.forEach((option) => this.observer.observe(option));
  }

  disconnect() {
    this.observer?.disconnect();
  }

  // No preventDefault: a link-based group must still navigate. For those the class change is
  // just the pressed state until the new page arrives with its own selection.
  select(event) {
    const chosen = event.currentTarget;
    this.optionTargets.forEach((option) => {
      const on = option === chosen;
      option.classList.toggle("is-on", on);
      if (option.hasAttribute("role")) {
        option.setAttribute("aria-checked", String(on));
        option.tabIndex = on ? 0 : -1;
      }
    });
    this.#moveThumb();
    this.dispatch("change", { detail: { value: chosen.dataset.value } });
  }

  // A single-choice group is arrowed through, not tabbed through.
  keys(event) {
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
    if (!step) return;

    const options = this.optionTargets;
    const next = options[(options.indexOf(event.currentTarget) + step + options.length) % options.length];
    event.preventDefault();
    next.focus();
    next.click();
  }

  #moveThumb() {
    if (!this.hasThumbTarget) return;

    const selected = this.optionTargets.find((option) => option.classList.contains("is-on")) || this.optionTargets[0];
    if (!selected) return;

    // First placement must not slide in from the left edge.
    if (!this.placed) this.thumbTarget.style.transition = "none";
    this.thumbTarget.style.left = `${selected.offsetLeft}px`;
    this.thumbTarget.style.width = `${selected.offsetWidth}px`;
    if (!this.placed) {
      this.thumbTarget.offsetHeight; // flush, so the next move animates
      this.thumbTarget.style.transition = "";
      this.placed = true;
    }
  }
}
