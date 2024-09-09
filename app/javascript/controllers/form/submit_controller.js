import { Controller } from "@hotwired/stimulus";
import debounce from "debounce";

// Submits a form

// Connects to data-controller="form--submit"
export default class extends Controller {
  submit() {
    this.element.requestSubmit();
  }
}
