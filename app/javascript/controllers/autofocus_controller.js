import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="autofocus"
export default class extends Controller {
  connect() {
    this.setCursorToEnd();
  }

  setCursorToEnd() {
    const input = this.element;
    input.focus();
    const length = input.value.length;
    input.setSelectionRange(length, length);
  }
}
