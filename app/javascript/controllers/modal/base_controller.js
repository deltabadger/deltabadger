import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="modal--base"
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
  // data-action="turbo:submit-end->modal--base#submitEnd"
  submitEnd(e) {
    this.frameAtSubmitEnd = document.getElementById('modal')
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
    if (this.frameAtSubmitEnd === undefined || this.frameAtSubmitEnd.innerHTML === frame.innerHTML) {
      console.log('fully closing modal')
      frame.removeAttribute("src")
      frame.innerHTML = ""
      document.body.classList.remove('overflow-hidden')
      this.#dispatchModalOpenEvent(false)
    } else {
      console.log('something was rendered in the modal while closing')
      const exitAnimationClass = this.frameAtSubmitEnd.dataset['hwAnimateOut'];
      if (exitAnimationClass) {
        console.log('removing previous exit animation class:', exitAnimationClass)
        frame.classList.remove(exitAnimationClass)
      }
    }
  }

  #dispatchModalOpenEvent(detail) {
    const newEvent = new CustomEvent('modalIsOpen', { detail });
    window.dispatchEvent(newEvent);
  }
}
