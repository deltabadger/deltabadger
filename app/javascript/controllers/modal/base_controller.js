import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="modal--base"
export default class extends Controller {
  static values = { closeUrl: String }

  connect() {
    this.#open()
    // needed because ESC key does not trigger close event
    this.enableBodyScrollBound = this.#enableBodyScroll.bind(this);
    this.element.addEventListener("close", this.enableBodyScrollBound)
  }

  disconnect() {
    this.element.removeEventListener("close", this.enableBodyScrollBound)
  }

  // hide modal on successful form submission
  // data-action="turbo:submit-end->modal--base#submitEnd"
  submitEnd(e) {
    if (e.detail.success) {
      this.animateOutCloseAndCleanUp()
    }
  }

  animateOutCloseAndCleanUp() {
    const frame = document.getElementById('modal')
    if (!frame) return

    // Support both the small modal (.modal) and the full-page wizard (.dialogview).
    const smallCard = frame.querySelector('.modal')
    const fullCard  = frame.querySelector('.dialogview__vertical-center')
    const card = smallCard || fullCard
    if (card) {
      card.classList.add(smallCard ? 'modal--closing' : 'dialogview__vertical-center--closing')
      this.element.classList.remove('dialog--open')
      this.element.classList.add('dialog--closing')

      let closeScheduled = false
      const close = () => {
        if (!closeScheduled) {
          closeScheduled = true
          this.#closeAndCleanUp()
        }
      }

      card.addEventListener('animationend', close, { once: true })
      setTimeout(close, 200)
    } else {
      this.#closeAndCleanUp()
    }
  }

  clickOutside(event) {
    if (event.target === this.element) {
      this.animateOutCloseAndCleanUp()
    }
  }

  #open() {
    this.element.show()
    requestAnimationFrame(() => this.element.classList.add('dialog--open'))
    document.body.classList.add('overflow-hidden')
    this.#dispatchModalOpenEvent(true)
  }

  #enableBodyScroll() {
    this.#closeAndCleanUp()
  }

  #closeAndCleanUp() {
    const closeUrl = this.hasCloseUrlValue ? this.closeUrlValue : null
    this.element.close()
    // clean up modal content
    const frame = document.getElementById('modal')
    frame.removeAttribute("src")
    frame.innerHTML = ""
    document.body.classList.remove('overflow-hidden')
    this.#dispatchModalOpenEvent(false)
    // Full-page dialogs navigate back to a known page after closing
    if (closeUrl) {
      Turbo.visit(closeUrl)
    }
  }

  #dispatchModalOpenEvent(detail) {
    const newEvent = new CustomEvent('modalIsOpen', { detail });
    window.dispatchEvent(newEvent);
  }
}
