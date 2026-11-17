import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="reveal-chrome"
// Pages rendered with content_for(:hide_chrome) start with the chrome hidden so a
// modal can open over a bare background. A modalIsOpen event with a falsy detail
// means the modal has closed, so the chrome comes back. On a page without the
// class, removing it is a no-op.
export default class extends Controller {
  connect() {
    window.addEventListener("modalIsOpen", this.#reveal)
  }

  disconnect() {
    window.removeEventListener("modalIsOpen", this.#reveal)
  }

  #reveal = (event) => {
    if (event.detail) return

    this.element.classList.remove("hide-chrome")
    window.removeEventListener("modalIsOpen", this.#reveal)
  }
}
