import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="modal--base"
export default class extends Controller {
  static values = { closeUrl: String }

  connect() {
    // An opener marked data-modal-return-focus (a grouped basket's index stack) takes the focus back on close; found again by
    // its id when a settings stream replaced it meanwhile.
    const opener = document.activeElement?.closest?.("[data-modal-return-focus]")
    if (opener) { this.opener = opener; this.openerId = opener.id }
    this.#open()
    // A click whose press began inside the card (a text selection dragged out) lands on the dialog
    // too; only a press that began on the backdrop closes it.
    this.pressBound = (e) => { this.pressedOn = e.target }
    this.element.addEventListener("pointerdown", this.pressBound)
    // needed because ESC key does not trigger close event
    this.enableBodyScrollBound = this.#enableBodyScroll.bind(this);
    this.element.addEventListener("close", this.enableBodyScrollBound)
    // A modal still open when the page is left (a turbo_stream redirect) would be cloned into Turbo's
    // snapshot and flash back on the next visit's preview or on Back.
    this.cleanUpBound = this.#cleanUp.bind(this)
    document.addEventListener("turbo:before-cache", this.cleanUpBound)
  }

  disconnect() {
    this.element.removeEventListener("pointerdown", this.pressBound)
    this.element.removeEventListener("close", this.enableBodyScrollBound)
    document.removeEventListener("turbo:before-cache", this.cleanUpBound)
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

    // The small modal (.modal) and the sheet card (.sheet, which zooms like it).
    const card = frame.querySelector('.modal, .sheet')
    if (card) {
      card.classList.add('modal--closing')
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
    if (event.target === this.element && this.pressedOn === this.element) {
      this.animateOutCloseAndCleanUp()
    }
  }

  #open() {
    this.element.show()
    // With nothing inside asking for focus, the dialog's focusing steps land on its first button, the sheet's close X,
    // which then draws its focus ring. The pane takes it instead, as Settings' autofocused pane does.
    if (document.activeElement?.matches(".sheet__close")) this.element.querySelector(".sheet__pane")?.focus({ preventScroll: true })
    requestAnimationFrame(() => this.element.classList.add('dialog--open'))
    this.#dispatchModalOpenEvent(true)
  }

  #enableBodyScroll() {
    this.#closeAndCleanUp()
  }

  #closeAndCleanUp() {
    const closeUrl = this.hasCloseUrlValue ? this.closeUrlValue : null
    this.#cleanUp()
    // Full-page dialogs navigate back to a known page after closing — unless that is the page
    // already behind the dialog, which would only re-render it.
    if (closeUrl && new URL(closeUrl, window.location.href).pathname !== window.location.pathname) {
      Turbo.visit(closeUrl)
    }
  }

  #cleanUp() {
    this.element.close()
    const frame = document.getElementById('modal')
    frame.removeAttribute("src")
    frame.innerHTML = ""
    this.#dispatchModalOpenEvent(false)
    const back = this.opener?.isConnected ? this.opener : (this.openerId && document.getElementById(this.openerId))
    back?.focus({ preventScroll: true })
  }

  #dispatchModalOpenEvent(detail) {
    const newEvent = new CustomEvent('modalIsOpen', { detail });
    window.dispatchEvent(newEvent);
  }
}
