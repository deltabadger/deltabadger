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
//
// When the options outgrow the room the parent gives them the whole thing folds into a menu —
// same track, same chip, now holding the selected label alone. That is measured rather than
// keyed to a breakpoint: the labels are translated and the option count depends on the data, so
// the width at which it stops fitting is not a number anyone could write down.
export default class extends Controller {
  static targets = ["thumb", "option"];

  connect() {
    // The chip is measured in pixels, so anything that reflows a label has to re-measure it.
    // A ResizeObserver on the options catches every cause at once — the container resizing, a
    // longer label in another locale, and above all the webfont swap: `document.fonts.ready`
    // resolves before the relayout, which leaves a chip sized to the fallback face sitting
    // over a narrower option. It also fires once on observe, which is the first placement.
    this.observer = new ResizeObserver(() => this.#layout());
    this.optionTargets.forEach((option) => this.observer.observe(option));
    // The room is the PARENT's, never the control's own: once collapsed the control is only as
    // wide as its trigger, and a control measuring itself would never learn that it fits again.
    this.observer.observe(this.element.parentElement);

    this.dismiss = (event) => {
      if (event.key === "Escape" || !this.element.contains(event.target)) this.#close();
    };
  }

  disconnect() {
    this.observer?.disconnect();
    this.#listen(false);
  }

  // Opening is the collapsed control's only job. A click on an option is that option's own
  // business — this just shuts the menu behind it.
  toggle(event) {
    if (!this.#collapsed) return;
    if (event.target.closest(".segmented__option")) return this.#close();

    this.element.classList.toggle("is-open");
    this.#listen(this.element.classList.contains("is-open"));
  }

  // No preventDefault: a link-based group must still navigate. For those the class change is
  // just the pressed state until the new page arrives with its own selection — which is why a
  // MODIFIED click is ignored: it opens a new tab and leaves this page as it was, so moving the
  // chip would put it on an option aria-current still says is not the one.
  select(event) {
    const chosen = event.currentTarget;
    if (chosen.href && (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button > 0)) return;
    this.optionTargets.forEach((option) => {
      const on = option === chosen;
      option.classList.toggle("is-on", on);
      if (option.hasAttribute("role")) {
        option.setAttribute("aria-checked", String(on));
        option.tabIndex = on ? 0 : -1;
      }
    });
    this.#layout();
    this.dispatch("change", { detail: { value: chosen.dataset.value } });
  }

  // A single-choice group is arrowed through, not tabbed through.
  keys(event) {
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
    if (!step) return;

    // Visible ones only: collapsed, the selected option is hidden from the menu because it is
    // the trigger, and arrowing onto something with no box focuses nothing.
    const options = this.optionTargets.filter((option) => option.offsetParent);
    const next = options[(options.indexOf(event.currentTarget) + step + options.length) % options.length];
    event.preventDefault();
    next.focus();
    next.click();
  }

  get #collapsed() {
    return this.element.classList.contains("segmented--collapsed");
  }

  #close() {
    this.element.classList.remove("is-open");
    this.#listen(false);
  }

  #listen(on) {
    const method = on ? "addEventListener" : "removeEventListener";
    document[method]("click", this.dismiss);
    document[method]("keydown", this.dismiss);
  }

  #layout() {
    // Only an expanded control can be asked how wide it wants to be, so the answer is kept.
    if (!this.#collapsed) this.natural = this.element.getBoundingClientRect().width;
    // Rounded, because clientWidth is: a parent that shrink-wraps this control reports exactly
    // its width, and a fraction of a pixel between the two is not a reason to fold anything.
    const collapse = Math.round(this.natural) > this.element.parentElement.clientWidth;

    // Only on the way in or out: opening the menu resizes the options, which lands back here,
    // and a close on every pass would shut the menu on the very click that opened it.
    if (this.#collapsed !== collapse) {
      this.element.classList.toggle("segmented--collapsed", collapse);
      this.#close();
    }
    this.#moveThumb();
  }

  #moveThumb() {
    if (!this.hasThumbTarget) return;

    const selected = this.optionTargets.find((option) => option.classList.contains("is-on")) || this.optionTargets[0];
    if (!selected) return;

    // Collapsed, the chip IS the trigger: it carries the selected label and sizes itself to it,
    // so the measured left and width have to go.
    if (this.#collapsed) {
      this.thumbTarget.textContent = selected.textContent.trim();
      this.thumbTarget.style.left = this.thumbTarget.style.width = "";
      return;
    }
    this.thumbTarget.textContent = "";

    // First placement must not slide in from the left edge.
    if (!this.placed) this.thumbTarget.style.transition = "none";
    // Rects, not offsetLeft/offsetWidth: those round to whole pixels, and a label half a pixel
    // wider than its slot pushes the chip that half pixel into the track's padding — visible as
    // a thinner gap on whichever side the chip sits.
    const track = this.element.getBoundingClientRect();
    const rect = selected.getBoundingClientRect();
    this.thumbTarget.style.left = `${rect.left - track.left}px`;
    this.thumbTarget.style.width = `${rect.width}px`;
    if (!this.placed) {
      this.thumbTarget.offsetHeight; // flush, so the next move animates
      this.thumbTarget.style.transition = "";
      this.placed = true;
    }
  }
}
