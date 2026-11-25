import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="window-drop"
// A file dragged anywhere over the window shows a blurred overlay; dropping it posts the form, whose
// answer is the import's own (a flash and the tracker, or the modal with its question or error).
//
// Only drags carrying files count, so text, links and tiles drag as before. dragenter/dragleave fire
// for every element crossed, so a depth count tells the window being left from a child being left.
// While a dialog is open (the import modal has its own drop zone) or a file is being sent, the window
// takes no drop, but still cancels the browser's default, which would open the file in place of the page.
export default class extends Controller {
  static targets = ["input"]

  connect() {
    this.depth = 0
    this.busy = false
  }

  enter(event) {
    if (!this.files(event)) return
    event.preventDefault()
    this.depth += 1
    if (this.taking()) this.element.classList.add("is-over")
  }

  over(event) {
    if (!this.files(event)) return
    event.preventDefault()
    // Over the import modal's own drop zone, its answer stands.
    if (event.target.closest && event.target.closest(".dropzone")) return
    event.dataTransfer.dropEffect = this.taking() ? "copy" : "none"
  }

  leave(event) {
    if (!this.files(event)) return
    this.depth = Math.max(0, this.depth - 1)
    if (this.depth === 0) this.element.classList.remove("is-over")
  }

  drop(event) {
    if (!this.files(event)) return
    // The import modal's drop zone handled it already, as it bubbled up.
    const handled = event.defaultPrevented
    event.preventDefault()
    const taking = this.taking()
    this.reset()
    if (handled || !taking) return
    const file = event.dataTransfer.files[0]
    if (!file) return

    const transfer = new DataTransfer()
    transfer.items.add(file)
    this.inputTarget.files = transfer.files
    this.busy = true
    this.element.classList.add("is-busy")
    this.element.requestSubmit()
  }

  reset() {
    this.depth = 0
    this.element.classList.remove("is-over")
  }

  done() {
    this.busy = false
    this.element.classList.remove("is-busy")
    this.inputTarget.value = ""
  }

  taking() {
    return !this.busy && !document.querySelector("dialog[open]")
  }

  files(event) {
    return Boolean(event.dataTransfer) && Array.from(event.dataTransfer.types || []).includes("Files")
  }
}
