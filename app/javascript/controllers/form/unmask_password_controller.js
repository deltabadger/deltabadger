import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--unmask-password"
export default class extends Controller {
  static targets = ["input"]

  toggle() {
    const type = this.inputTarget.getAttribute("type") === "password" ? "text" : "password"
    this.inputTarget.setAttribute("type", type)
  }
}
