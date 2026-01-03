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
    if (e.detail.success) {
      this.animateOutCloseAndCleanUp()
    }
  }

  animateOutCloseAndCleanUp() {
    const frame = document.getElementById('modal')
    if (!frame) return
    
    const elementToRemove = frame.firstElementChild
    if (!elementToRemove) return
    
    const exitAnimationClass = elementToRemove.dataset['hwAnimateOut'];
    if (exitAnimationClass) {
      elementToRemove.classList.remove(exitAnimationClass);
      
      // Listen for animationend, but also use a timeout as fallback
      let closeScheduled = false;
      const close = () => {
        if (!closeScheduled) {
          closeScheduled = true;
          this.#closeAndCleanUp();
        }
      };
      
      elementToRemove.addEventListener("animationend", close, { once: true })
      
      // Force browser to process the removal
      void elementToRemove.offsetWidth;
      
      // Re-add the class to trigger the animation
      elementToRemove.classList.add(exitAnimationClass)
      
      // Fallback timeout: animation is 0.25s, add a bit of buffer
      setTimeout(close, 300)
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
