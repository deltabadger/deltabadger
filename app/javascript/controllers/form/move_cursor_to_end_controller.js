import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--move-cursor-to-end"
export default class extends Controller {
  static targets = ["input"]

  connect() {
    this.#moveCursorToEnd();
  }

  #moveCursorToEnd() {
    this.inputTargets.forEach(input => {
      // Temporarily change type to text
      const originalType = input.type;
      input.type = "text";
      const valueLength = input.value.length;
      input.setSelectionRange(valueLength, valueLength);
      // Revert type to number
      input.type = originalType;
    });
  }
}
