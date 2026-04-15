import { Controller } from "@hotwired/stimulus";

// Manages .active / .filled on each slot form inside the sentence.
//   - .active moves to the clicked slot
//   - .filled is toggled on each slot form based on whether its input has a
//     value (reactive to typing, additive to server-rendered filled chips).
//
// Connects to data-controller="conversational-sentence" on .conversational.
export default class extends Controller {
  connect() {
    this.element.querySelectorAll("form").forEach((form) => this.#syncFilled(form));
  }

  activate(event) {
    const form = event.target.closest("form");
    if (!form || !this.element.contains(form)) return;
    this.element.querySelectorAll(".active").forEach((el) => {
      if (el !== form) el.classList.remove("active");
    });
    form.classList.add("active");
    // Any click on a non-editable slot submits its form:
    //   - filled (readonly input) → GET back-nav URL
    //   - optional (no name)      → POST promote_to_dual
    // The active editable form contains <input name="query">; its own
    // form--submit-after-delay submits on typing, so skip submission here.
    const editable = form.querySelector('input[name="query"]');
    if (!editable) form.requestSubmit();
  }

  updateFilled(event) {
    const form = event.target.form;
    if (!form || !this.element.contains(form)) return;
    this.#syncFilled(form);
  }

  #syncFilled(form) {
    const input = form.querySelector("input:not([type=hidden])");
    if (!input) return;
    form.classList.toggle("filled", input.value !== "");
  }
}
