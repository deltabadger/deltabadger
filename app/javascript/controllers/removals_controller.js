import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="removals"
export default class extends Controller {
  remove() {
    this.element.remove()
  }

  submitAndRemove(event) {
    event.preventDefault()
    const form = this.element.querySelector('form');
    form.requestSubmit();
    this.element.remove();
  }
}
