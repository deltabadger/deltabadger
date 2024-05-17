import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form-key-press"
export default class extends Controller {
  static targets = ["button", "input"];

  connect() {
    this.handleModalIsOpenBound = this.handleModalIsOpen.bind(this);
    this.handleKeyPressBound = this.handleKeyPress.bind(this);
    window.addEventListener("modalIsOpen", this.handleModalIsOpenBound);
    window.addEventListener("keydown", this.handleKeyPressBound);
  }

  disconnect() {
    window.removeEventListener("modalIsOpen", this.handleModalIsOpenBound);
    window.removeEventListener("keydown", this.handleKeyPressBound);
  }
  
  handleModalIsOpen(event) {
    if (event.detail) {
      window.removeEventListener("keydown", this.handleKeyPressBound);
    } else {
      window.addEventListener("keydown", this.handleKeyPressBound);
    }
  }

  handleKeyPress(event) {
    const waitForInput = () => {

      // Dynamically find the input target after the turbo frame has loaded
      const inputElement = document.querySelector('[data-form-key-press-target="input"]');

      if (inputElement) {
        inputElement.value = event.key;
      } else {
        setTimeout(waitForInput, 10);
      }
    };

    if (this.isLetter(event)) {
      this.buttonTarget.click();
      waitForInput();
    }
  }

  isLetter(event) {
    const key = event.key;
    const hasModifier = event.ctrlKey || event.altKey || event.shiftKey || event.metaKey;
    return !hasModifier && key.length === 1 && key.match(/[a-z]/i);
  }
}
