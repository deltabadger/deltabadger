import { Controller } from "@hotwired/stimulus";

// On click of an input.ticker inside the sentence, move .active onto it and
// off any sibling ticker.
//
// Connects to data-controller="conversational-sentence" on .conversational.
export default class extends Controller {
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
}
