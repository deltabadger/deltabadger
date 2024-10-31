import { Controller } from "@hotwired/stimulus";

// Submits a form

// Connects to data-controller="form--submit"
export default class extends Controller {
  submit() {
    console.log("Submitting", this.element);
    this.element.requestSubmit();
  }
}
