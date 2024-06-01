import { Controller } from "@hotwired/stimulus";
import debounce from "debounce";

// Automatically submits a form after a delay

// Connects to data-controller="form-submission"
export default class extends Controller {
  initialize() {
    this.submit = debounce(this.submit.bind(this), 300);
  }

  submit() {
    this.element.requestSubmit();
  }
}
