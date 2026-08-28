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
//
// The trailing action can carry a list of its own. Expanded that list is a box hanging off the
// action; collapsed it is the dropdown's second pane, which slides in over the options and back
// out again — one box, two panes, so the reader never loses the thing they opened.
//
// Given a `data-segmented-key` it also REMEMBERS: the last choice comes back on the next visit,
// through the same select path a click takes, so every consumer hears about it the only way it
// knows how. Naming the key is what opts a control in — see the partial for why a control gets
// one or does not.
export default class extends Controller {
  static targets = ["thumb", "option", "dropdown"];

  connect() {
    this.dismiss = (event) => {
      if (event.key !== "Escape" && this.element.contains(event.target)) return;
      // Escape is dismissed from the keyboard, with the cursor inside the very thing about to be
      // hidden, so it has to be handed back somewhere visible. A click elsewhere has already put
      // the cursor where the reader wanted it, and taking it back would be the rude version.
      const stranded = event.key === "Escape" && this.element.contains(document.activeElement);
      this.#close();
      if (stranded) this.#action?.focus();
    };
    // Turbo restores the DOM exactly as it was cached — open menu, slid pane, measured height and
    // all. A menu the reader left open on the page they navigated away from is not a state to
    // come back into, and #layout only resets it when the fold itself changes.
    this.#close();

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

    // Last, so the click it may fire lands on a control that is wired and measured.
    this.#restore();
  }

  disconnect() {
    this.observer?.disconnect();
    this.#listen(false);
  }

  // Opening is the collapsed control's only job. A click on anything inside is that control's
  // own business — this only decides what happens to the box behind it.
  toggle(event) {
    if (!this.#collapsed) return;
    // The action slides the box to its own list and back slides it home. Both leave it open.
    if (event.target.closest(".segmented__option--action, .segmented__back")) return;
    // Everything else in here navigates, so the box has done its job.
    if (event.target.closest(".segmented__option, .segmented__submenu-item")) return this.#close();

    this.element.classList.toggle("is-open");
    this.#listen(this.element.classList.contains("is-open"));
    this.#size();
  }

  // The action OPENS its list rather than following its href — and puts it away again, because
  // the control the reader opened it with is the one they reach for to close it.
  open(event) {
    event.preventDefault();
    if (this.element.classList.contains("is-submenu")) return this.back();

    this.element.classList.add("is-submenu");
    // Expanded, that list is a box of its own, so the options menu has nothing to do with it.
    if (!this.#collapsed) this.element.classList.remove("is-open");
    event.currentTarget.setAttribute("aria-expanded", "true");
    this.#listen(true);
    this.#size();
  }

  // Back to the options. Focus comes with it: the pane it was in is only slid out of sight, and
  // a cursor parked there is one the reader cannot see. Dismissing still listens while the
  // options menu itself is open — closing the list is not closing the menu.
  back() {
    this.element.classList.remove("is-submenu");
    this.#action?.setAttribute("aria-expanded", "false");
    this.#listen(this.element.classList.contains("is-open"));
    this.#size();
    this.#action?.focus();
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
    this.#remember(chosen.dataset.value);
    this.dispatch("change", { detail: { value: chosen.dataset.value } });
  }

  // --- memory -----------------------------------------------------------------------
  //
  // A CLICK, not a state assignment: the choice has to travel the same path a real one does, or
  // the chart would come back on a mode its curve is not drawn in and the log on a tab its rows
  // are not filtered to. Everything downstream is already listening for that.
  //
  // A remembered value whose option is not here is no value at all — a bot's log loses the tab
  // its last order left, a range outlives the history it fitted — and the server's own default
  // stands untouched.
  #restore() {
    const stored = this.#stored;
    if (!stored) return;

    const option = this.optionTargets.find((target) => target.dataset.value === stored);
    if (option && !option.classList.contains("is-on")) option.click();
  }

  get #stored() {
    if (!this.element.dataset.segmentedKey) return null;

    try {
      return localStorage.getItem(`segmented:${this.element.dataset.segmentedKey}`);
    } catch {
      return null; // storage can be denied outright (private mode, embedded webview)
    }
  }

  #remember(value) {
    if (!this.element.dataset.segmentedKey) return;

    try {
      localStorage.setItem(`segmented:${this.element.dataset.segmentedKey}`, value);
    } catch {
      // Not being able to remember a choice is not a reason to fail making it.
    }
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
    this.element.classList.remove("is-open", "is-submenu");
    this.#action?.setAttribute("aria-expanded", "false");
    this.#listen(false);
    this.#size();
  }

  get #action() {
    return this.element.querySelector(".segmented__option--action[aria-expanded]");
  }

  // The collapsed box holds one pane at a time and the two are different heights, so it cannot
  // be sized by its content: `auto` does not animate, and the taller pane would leave the
  // shorter one sitting in a half-empty box. Measured in pixels, like the chip.
  #size() {
    if (!this.hasDropdownTarget) return;

    const sub = this.element.classList.contains("is-submenu");
    const menu = this.element.querySelector(".segmented__menu");
    const submenu = this.element.querySelector(".segmented__submenu");
    // The pane that is off screen is CLIPPED, not hidden, so without this its links stay in the
    // tab order behind the pane that is on screen — and a reader tabbing through them is
    // focusing things nobody can see. Expanded there is only ever one of the two on screen.
    submenu?.toggleAttribute("inert", !sub);
    menu.toggleAttribute("inert", this.#collapsed && sub);

    const pane = sub ? submenu : menu;
    const showing = this.#collapsed && (sub || this.element.classList.contains("is-open"));
    this.dropdownTarget.style.height = showing && pane ? `${pane.offsetHeight}px` : "";
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
    const collapse = Math.round(this.natural) > this.#room();

    // Only on the way in or out: opening the menu resizes the options, which lands back here,
    // and a close on every pass would shut the menu on the very click that opened it.
    if (this.#collapsed !== collapse) {
      this.element.classList.toggle("segmented--collapsed", collapse);
      this.#close();
    }
    this.#moveThumb();
  }

  // The room this control actually has: its parent's width, less anything else standing in that
  // row and the gaps between. A control that measures the whole line while a switch shares it
  // believes it fits, stays expanded, and overflows — which is only ever visible at the width
  // where it was about to fold.
  #room() {
    const parent = this.element.parentElement;
    const gap = parseFloat(getComputedStyle(parent).columnGap) || 0;
    return [...parent.children].reduce(
      (room, child) => (child === this.element ? room : room - child.getBoundingClientRect().width - gap),
      parent.clientWidth
    );
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
