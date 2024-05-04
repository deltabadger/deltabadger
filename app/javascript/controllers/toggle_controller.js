import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="toggle"
export default class extends Controller {
  static targets = ["hey"];
  static values = { isOpened: { type: Boolean, default: true } };

  connect() {
    console.log("T Connected!");
    console.log(this.element);
  }

  menu() {
    console.log("T Menu!");
    console.log(this.heyTarget);
    this.isOpenedValue ? this.hide() : this.show();
    this.isOpenedValue = !this.isOpenedValue;
  }

  show() {
    this.heyTarget.style.display = "block";
  }

  hide() {
    this.heyTarget.style.display = "none";
  }
}
