import { Controller } from "@hotwired/stimulus";

export default class extends Controller {
  static targets = ["toggleable"]
  static values = {
    class: { type: String, default: "show-tooltip" },
    dynamicDelay: { type: Number, default: 500 }
  }
  static hoverTimeout = null;
  static noHoverTimeout = null;

  connect() {
    this.showTimeout = null;
  }

  toggle() {
    this.element.classList.toggle(this.classValue);
    this.#handlePasswordToggling(this.element);
  }

  toggleTarget(event) {
    if (this.hasToggleableTarget) {
      this.toggleableTarget.classList.toggle(this.classValue);
      
      // Handle password input toggling if it's a password field
      if (this.toggleableTarget.getAttribute("type") === "password" || 
          this.toggleableTarget.getAttribute("type") === "text") {
        this.#togglePasswordVisibility(this.toggleableTarget);
      }
    } else {
      this.element.classList.toggle(this.classValue);
      this.#handlePasswordToggling(this.element);
    }
  }

  show() {
    this.#clearTimeouts();
    this.constructor.hoverTimeout = setTimeout(() => {
      this.dynamicDelayValue = 0;
    }, 500);

    this.#setTimeout();
  }

  hide() {
    this.#clearTimeouts();
    
    if (this.hasToggleableTarget) {
      this.toggleableTarget.classList.remove(this.classValue);
    } else {
      this.element.classList.remove(this.classValue);
    }

    this.constructor.noHoverTimeout = setTimeout(() => {
      this.dynamicDelayValue = 500;
    }, 500);
  }

  #clearTimeouts() {
    clearTimeout(this.showTimeout);
    clearTimeout(this.constructor.hoverTimeout);
    clearTimeout(this.constructor.noHoverTimeout);
  }

  #setTimeout() {
    this.showTimeout = setTimeout(() => {
      if (this.hasToggleableTarget) {
        this.toggleableTarget.classList.add(this.classValue);
      } else {
        this.element.classList.add(this.classValue);
      }
    }, this.dynamicDelayValue);
  }

  #handlePasswordToggling(element) {
    // Find all password inputs and toggle them
    const passwordInputs = element.querySelectorAll('input[type="password"], input[type="text"]');
    passwordInputs.forEach(input => {
      this.#togglePasswordVisibility(input);
    });
  }

  #togglePasswordVisibility(input) {
    if (input.getAttribute("type") === "password") {
      input.setAttribute("type", "text");
    } else if (input.getAttribute("type") === "text") {
      input.setAttribute("type", "password");
    }
  }
} 