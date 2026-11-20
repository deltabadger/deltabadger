import { Controller } from "@hotwired/stimulus"

// D-LANGUAGE-STEP: picking a row puts its greeting in the heading, its button label on the button and its code on
// <html lang>, at once and without a request. The rows are the server's; without JavaScript the page still submits.
// The list's bottom fade goes once the list is scrolled to its end.
export default class extends Controller {
  static targets = ["hello", "button", "card", "list"]

  connect() { this.scrolled() }

  pick(event) {
    const row = event.target.closest("[data-greeting]")
    if (!row) return
    this.helloTarget.textContent = row.dataset.greeting
    this.buttonTarget.textContent = row.dataset.continue
    document.documentElement.lang = event.target.value
  }

  scrolled() {
    const list = this.listTarget
    this.cardTarget.classList.toggle("is-end", list.scrollTop + list.clientHeight >= list.scrollHeight - 4)
  }
}
