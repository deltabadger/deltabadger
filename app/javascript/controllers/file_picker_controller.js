import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="file-picker"
// Opens a hidden file input from a visible button, and submits the form once a
// file has been chosen.
//
// <form data-controller="file-picker">
//   <button type="button" data-action="click->file-picker#open">Import</button>
//   <input type="file" hidden
//          data-file-picker-target="input"
//          data-action="change->file-picker#submit">
// </form>
//
// As a DROP ZONE, the click needs no JavaScript at all: a <label> wrapping the input already opens
// the picker anywhere on it, which is also what makes the whole area reachable by keyboard. Only
// the dragging needs handlers — `dragover` must be cancelled or the browser navigates to the file
// instead of letting the page have it.
//
// <label data-controller="file-picker"
//        data-action="dragover->file-picker#hover dragleave->file-picker#unhover drop->file-picker#drop">
//   <input type="file" data-file-picker-target="input" data-action="change->file-picker#show">
//   <span data-file-picker-target="name"></span>
// </label>
export default class extends Controller {
  static targets = ["input", "name"]
  static classes = ["over", "filled", "busy"]

  open() {
    this.inputTarget.click()
  }

  // A chosen file IS the instruction — there is nothing to confirm afterwards, so nothing waits for
  // a button. `requestSubmit` rather than `submit`: it fires the submit event, which is what lets
  // Turbo take the request; `form.submit()` bypasses Turbo entirely.
  submit() {
    this.show()
    this.element.classList.add(this.busyClassName)
    const form = this.inputTarget.form
    form.requestSubmit ? form.requestSubmit() : form.submit()
  }

  hover(event) {
    event.preventDefault()
    this.element.classList.add(this.overClassName)
  }

  unhover() {
    this.element.classList.remove(this.overClassName)
  }

  drop(event) {
    event.preventDefault()
    this.unhover()
    const files = event.dataTransfer && event.dataTransfer.files
    if (!files || files.length === 0) return

    // Assigning the FileList straight across is what makes the dropped file the one the form
    // submits — without it the zone would look like it took the file and post nothing. Assigning it
    // fires no `change`, so the send is explicit here.
    this.inputTarget.files = files
    this.submit()
  }

  // The only feedback that the file arrived. A drop leaves no trace otherwise: the native input is
  // hidden, so its "1 file selected" text is hidden with it.
  show() {
    const file = this.inputTarget.files && this.inputTarget.files[0]
    if (this.hasNameTarget) this.nameTarget.textContent = file ? file.name : ""
    this.element.classList.toggle(this.filledClassName, Boolean(file))
  }

  get overClassName() {
    return this.hasOverClass ? this.overClass : "is-over"
  }

  get filledClassName() {
    return this.hasFilledClass ? this.filledClass : "has-file"
  }

  get busyClassName() {
    return this.hasBusyClass ? this.busyClass : "is-busy"
  }
}
