import { Controller } from "@hotwired/stimulus"

// D-LANGUAGE-STEP: picking a row crossfades the heading to its greeting and the button to its label, puts and its code on
// <html lang>, at once and without a request. The rows are the server's; without JavaScript the page still submits.
// The list's bottom fade goes once the list is scrolled to its end.
export default class extends Controller {
  static targets = ["hello", "button", "card", "list"]

  connect() { this.scrolled() }

  pick(event) {
    const row = event.target.closest("[data-greeting]")
    if (!row) return
    this.#crossfade(this.helloTarget, row.dataset.greeting)
    this.#crossfade(this.buttonTarget, row.dataset.continue)
    document.documentElement.lang = event.target.value
  }

  // The old text fades out under the new one fading in (both sit in the same grid cell, `.crossfade`).
  #crossfade(holder, text) {
    const old = holder.lastElementChild
    if (old.textContent === text) return
    const next = document.createElement("span")
    next.textContent = text
    holder.append(next)
    const fade = { duration: matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 150, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" }
    next.animate([{ opacity: 0 }, { opacity: 1 }], fade)
    for (const gone of [...holder.children].slice(0, -1)) {
      gone.animate([{ opacity: getComputedStyle(gone).opacity }, { opacity: 0 }], { ...fade, fill: "forwards" }).finished.then(() => gone.remove())
    }
  }

  scrolled() {
    const list = this.listTarget
    this.cardTarget.classList.toggle("is-end", list.scrollTop + list.clientHeight >= list.scrollHeight - 4)
  }
}
