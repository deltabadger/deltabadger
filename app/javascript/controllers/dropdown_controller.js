import { Controller } from "@hotwired/stimulus"

export default class extends Controller {
  static targets = ["wrapper"];

  connect() {
    this.handleOutsideClick = this.handleOutsideClick.bind(this);
  }

  toggle() {
    this.wrapperTarget.classList.toggle("dropdown-wrapper--on");

    if (this.wrapperTarget.classList.contains("dropdown-wrapper--on")) {
      document.addEventListener("click", this.handleOutsideClick);
    } else {
      document.removeEventListener("click", this.handleOutsideClick);
    }
  }

  handleOutsideClick(event) {
    if (!this.wrapperTarget.contains(event.target)) {
      this.wrapperTarget.classList.remove("dropdown-wrapper--on");
      document.removeEventListener("click", this.handleOutsideClick);
    }
  }
}