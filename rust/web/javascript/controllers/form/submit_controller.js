import { Controller } from "@hotwired/stimulus";

// Submits a form. When a `trigger` target is present, uses it as the submitter
// so the form adopts its formaction/formmethod/formnovalidate attributes.

// Connects to data-controller="form--submit"
export default class extends Controller {
  static targets = ["trigger"]

  submit() {
    if (this.hasTriggerTarget) {
      this.element.requestSubmit(this.triggerTarget);
    } else {
      this.element.requestSubmit();
    }
  }
}
