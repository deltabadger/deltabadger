import { Controller } from "@hotwired/stimulus";

export default class extends Controller {
  static targets = ["toggleable"]
  static values = {
    class: { type: String, default: "show-tooltip" },
    dynamicDelay: { type: Number, default: 500 },
    outsideClickHandler: { type: Boolean, default: false }
  }
  static hoverTimeout = null;
  static noHoverTimeout = null;

  connect() {
    this.showTimeout = null;
    this.handleOutsideClick = this.handleOutsideClick.bind(this);
  }

  toggle() {
    this.element.classList.toggle(this.classValue);
    this.#handlePasswordToggling(this.element);
    
    // Handle outside click for dropdowns or other elements that need it
    if (this.outsideClickHandlerValue) {
      if (this.element.classList.contains(this.classValue)) {
        document.addEventListener("click", this.handleOutsideClick);
      } else {
        document.removeEventListener("click", this.handleOutsideClick);
      }
    }
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
    
    // Handle outside click for dropdowns or other elements that need it
    if (this.outsideClickHandlerValue) {
      if ((this.hasToggleableTarget && this.toggleableTarget.classList.contains(this.classValue)) ||
          (!this.hasToggleableTarget && this.element.classList.contains(this.classValue))) {
        document.addEventListener("click", this.handleOutsideClick);
      } else {
        document.removeEventListener("click", this.handleOutsideClick);
      }
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

  handleOutsideClick(event) {
    const targetElement = this.hasToggleableTarget ? this.toggleableTarget : this.element;
    
    if (!this.element.contains(event.target)) {
      targetElement.classList.remove(this.classValue);
      document.removeEventListener("click", this.handleOutsideClick);
    }
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