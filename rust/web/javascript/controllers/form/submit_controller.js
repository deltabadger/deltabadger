import { Controller } from "@hotwired/stimulus";

// Submits a form. When a `trigger` target is present, uses it as the submitter
// so the form adopts its formaction/formmethod/formnovalidate attributes.
//
// Fired by a segmented control's change (the menu's display currency), it waits for the chip's slide
// to end first: the response redraws the page, which moves the menu and cuts a running transition
// short. A second pick while waiting replaces the first, so only the last one is sent.

// Connects to data-controller="form--submit"
export default class extends Controller {
  static targets = ["trigger"]

  submit(event) {
    const thumb = event?.type === "segmented:change" && event.target.querySelector(".segmented__thumb");
    if (!thumb) return this.#send();
    this.#cancel?.();
    let timer;
    const done = () => { this.#cancel(); this.#send(); };
    thumb.addEventListener("transitionend", done);
    timer = setTimeout(done, 300); // no transition (reduced motion, a collapsed control): don't wait for one
    this.#cancel = () => { clearTimeout(timer); thumb.removeEventListener("transitionend", done); this.#cancel = null; };
  }

  #cancel = null;

  #send() {
    if (this.hasTriggerTarget) {
      this.element.requestSubmit(this.triggerTarget);
    } else {
      this.element.requestSubmit();
    }
  }
}
