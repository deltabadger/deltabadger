import { Controller } from "@hotwired/stimulus"

// A switch between index and portfolio derives the new composition before it answers, which takes a
// moment. While the request runs, a modal says so on top of whatever was clicked. Success redirects,
// replacing the page and the modal with it; a refusal closes it so the flash reads on its own.
export default class extends Controller {
  static targets = ["template"]

  show() {
    this.dialog = this.templateTarget.content.firstElementChild.cloneNode(true)
    this.dialog.addEventListener("cancel", (event) => event.preventDefault()) // no Escape mid-request
    document.body.append(this.dialog)
    this.dialog.showModal()
  }

  hide(event) {
    if (event.detail.success) return

    this.dialog?.remove()
    this.dialog = null
  }

  disconnect() {
    this.dialog?.remove()
  }
}
