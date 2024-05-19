import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="modals"
export default class extends Controller {
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
  // data-action="turbo:submit-end->modals#submitEnd"
  submitEnd(e) {
    if (e.detail.success) {
      this.animateOutCloseAndCleanUp()
    }
  }

  animateOutCloseAndCleanUp() {
    const frame = document.getElementById('modal')
    const elementToRemove = frame.firstElementChild
    const exitAnimationClass = elementToRemove.dataset['hwAnimateOut'];
    if (exitAnimationClass) {
      elementToRemove.classList.add(exitAnimationClass)
      elementToRemove.addEventListener("animationend", this.#closeAndCleanUp.bind(this))
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
    this.element.showModal()
    document.body.classList.add('overflow-hidden')
    this.#dispatchModalOpenEvent(true)
  }

  #enableBodyScroll() {
    this.#closeAndCleanUp()
  }

  #closeAndCleanUp() {
    this.element.close()
    // clean up modal content
    const frame = document.getElementById('modal')
    frame.removeAttribute("src")
    frame.innerHTML = ""
    document.body.classList.remove('overflow-hidden')
    this.#dispatchModalOpenEvent(false)
  }

  #dispatchModalOpenEvent(detail) {
    const newEvent = new CustomEvent('modalIsOpen', { detail });
    window.dispatchEvent(newEvent);
  }
}
