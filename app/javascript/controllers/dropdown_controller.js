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
      document.addEventListener("click", this.handleOutsideClick);
    } else {
      document.removeEventListener("click", this.handleOutsideClick);
    }
  }

  handleOutsideClick(event) {
    if (!this.wrapperTarget.contains(event.target)) {
      this.wrapperTarget.classList.remove("dropdown-wrapper--on");
      if (this.hasTriggerTarget && this.hasActiveClass) {
        this.triggerTarget.classList.remove(this.activeClass);
      }
      document.removeEventListener("click", this.handleOutsideClick);
    }
  }
}