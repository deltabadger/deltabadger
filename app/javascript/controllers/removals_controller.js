import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="removals"
export default class extends Controller {
  connect() {
    if (this.element.classList.contains('flash__message')) {
      const siblings = this.element.parentElement.querySelectorAll('.flash__message')
      const all = Array.from(siblings)
      const reverseIndex = all.length - 1 - all.indexOf(this.element)

      this.duration = 5000 + reverseIndex * 3000
      this.remaining = this.duration

      this.startAutoClose()

      this.element.addEventListener('mouseenter', this.pause)
      this.element.addEventListener('mouseleave', this.resume)
    }
  }

  disconnect() {
    clearTimeout(this.timeout)
    this.element.removeEventListener('mouseenter', this.pause)
    this.element.removeEventListener('mouseleave', this.resume)
  }

  startAutoClose() {
    this.startedAt = Date.now()
    this.timeout = setTimeout(() => this.fadeOut(), this.remaining)
  }

  fadeOut() {
    this.element.classList.add('flash--fading')
    this.element.addEventListener('transitionend', () => this.element.remove(), { once: true })
  }

  pause = () => {
    clearTimeout(this.timeout)
    this.remaining -= Date.now() - this.startedAt
  }

  resume = () => {
    this.startAutoClose()
  }

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
