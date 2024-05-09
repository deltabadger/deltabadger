import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="modals"
export default class extends Controller {
  connect() {
    this.open()
    // needed because ESC key does not trigger close event
    this.element.addEventListener("close", this.enableBodyScroll.bind(this))
  }

  disconnect() {
    this.element.removeEventListener("close", this.enableBodyScroll.bind(this))
  }

  // hide modal on successful form submission
  // data-action="turbo:submit-end->modals#submitEnd"
  submitEnd(e) {
    if (e.detail.success) {
      this.close()
    }
  }

  open() {
    this.element.showModal()
    document.body.classList.add('overflow-hidden')
  }

  close() {
    const frame = document.getElementById('modal')
    const elementToRemove = frame.firstElementChild
    const exitAnimationClass = elementToRemove.dataset['hwAnimateOut'];
    if (exitAnimationClass) {
      elementToRemove.classList.add(exitAnimationClass)
      elementToRemove.addEventListener("animationend", this.closeAndCleanUp.bind(this))
    } else {
      this.closeAndCleanUp()
    }
  }

  enableBodyScroll() {
    document.body.classList.remove('overflow-hidden')
  }

  clickOutside(event) {
    if (event.target === this.element) {
      this.close()
    }
  }

  closeAndCleanUp() {
    this.element.close()
    // clean up modal content
    const frame = document.getElementById('modal')
    frame.removeAttribute("src")
    frame.innerHTML = ""
  }
}
