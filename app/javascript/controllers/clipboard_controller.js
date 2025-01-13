import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="clipboard"
export default class extends Controller {
  static targets = ["input", "alert"];

  copy() {
    const input = this.inputTarget;

    try {
      navigator.clipboard.writeText(input.value);
      this.#showAlert();
    } catch (error) {
      console.error("Clipboard copy failed", error);
    }
  }

  #showAlert() {
    const alert = this.alertTarget;
    alert.style.visibility = "visible";

    setTimeout(() => {
      alert.style.visibility = "hidden";
    }, 2000);
  }
}
