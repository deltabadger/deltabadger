import { Controller } from "@hotwired/stimulus"

export default class extends Controller {
  static targets = ["wrapper", "trigger"];
  static classes = ["active"];

  connect() {
    this.handleOutsideClick = this.handleOutsideClick.bind(this);
  }

  toggle() {
    this.wrapperTarget.classList.toggle("dropdown-wrapper--on");
    const isOpen = this.wrapperTarget.classList.contains("dropdown-wrapper--on");

    if (this.hasTriggerTarget && this.hasActiveClass) {
      this.triggerTarget.classList.toggle(this.activeClass, isOpen);
    }

    if (isOpen) {
      this.#hideTooltip();
      document.addEventListener("click", this.handleOutsideClick);
    } else {
      this.#showTooltip();
      document.removeEventListener("click", this.handleOutsideClick);
    }
  }

  handleOutsideClick(event) {
    if (!this.wrapperTarget.contains(event.target)) {
      this.wrapperTarget.classList.remove("dropdown-wrapper--on");
      if (this.hasTriggerTarget && this.hasActiveClass) {
        this.triggerTarget.classList.remove(this.activeClass);
      }
      this.#showTooltip();
      document.removeEventListener("click", this.handleOutsideClick);
    }
  }

  #hideTooltip() {
    const tooltip = this.wrapperTarget.querySelector(".tooltip");
    if (tooltip) tooltip.style.display = "none";
  }

  #showTooltip() {
    const tooltip = this.wrapperTarget.querySelector(".tooltip");
    if (tooltip) tooltip.style.display = "";
  }
}