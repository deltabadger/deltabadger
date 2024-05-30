import { Controller } from "@hotwired/stimulus"

// Used to automatically search on key press, similar to TradingView.
// Listens to any key press that is a letter, clicks the button, and sets the input value to the key pressed

// Connects to data-controller="form-key-press"
export default class extends Controller {
  static targets = ["button"];

  connect() {
    this.keysPressed = [];
    this.handleModalIsOpenBound = this.#handleModalIsOpen.bind(this);
    this.handleKeyPressBound = this.#handleKeyPress.bind(this);
    window.addEventListener("modalIsOpen", this.handleModalIsOpenBound);
    window.addEventListener("keydown", this.handleKeyPressBound);
  }

  disconnect() {
    window.removeEventListener("modalIsOpen", this.handleModalIsOpenBound);
    window.removeEventListener("keydown", this.handleKeyPressBound);
  }

  #handleModalIsOpen(event) {
    if (event.detail) {
      if (this.keysPressed.length === 0) {
        window.removeEventListener("keydown", this.handleKeyPressBound);
      }
    } else {
      window.addEventListener("keydown", this.handleKeyPressBound);
    }
  }

  #handleKeyPress(event) {

    const waitForInputAndFillItWithKeysPressed = () => {

      // Dynamically find the input target after the turbo frame has loaded
      const formElement = document.querySelector('[data-form-key-press-target="form"]');
      const inputElement = document.querySelector('[data-form-key-press-target="input"]');

      if (inputElement) {
        inputElement.value = this.keysPressed.join("");
        this.keysPressed = [];
        window.removeEventListener("keydown", this.handleKeyPressBound);
        formElement.requestSubmit();
      } else {
        setTimeout(waitForInputAndFillItWithKeysPressed, 50);
      }
    };

    if (this.#isLetter(event)) {
      this.keysPressed.push(event.key);
      if (this.keysPressed.length === 1) {
        this.buttonTarget.click();
        waitForInputAndFillItWithKeysPressed();
      }
    }
  }

  #isLetter(event) {
    const key = event.key;
    const hasModifier = event.ctrlKey || event.altKey || event.shiftKey || event.metaKey;
    return !hasModifier && key.length === 1 && key.match(/[a-z]/i);
  }
}
