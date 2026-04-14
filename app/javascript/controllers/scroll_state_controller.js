import { Controller } from "@hotwired/stimulus";

// Toggles a `.scrolled` class on the element based on whether it has been
// scrolled down from the top. Lets CSS react to scroll state (e.g. sticky
// shadows, hiding a "scroll for more" hint).
//
// Connects to data-controller="scroll-state"
export default class extends Controller {
  connect() {
    this.element.addEventListener("scroll", this.#update, { passive: true });
    this.#update();
  }

  disconnect() {
    this.element.removeEventListener("scroll", this.#update);
  }

  #update = () => {
    this.element.classList.toggle("scrolled", this.element.scrollTop > 0);
  };
}
