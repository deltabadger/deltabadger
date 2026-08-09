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
export default class extends Controller {
  static targets = ["input"]

  open() {
    this.inputTarget.click()
  }

  submit() {
    this.inputTarget.form.submit()
  }
}
