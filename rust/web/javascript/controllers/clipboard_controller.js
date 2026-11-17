import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="clipboard"
// Copies a literal value to the clipboard.
//
// <span data-controller="clipboard"
//       data-clipboard-text-value="https://example.com"
//       data-action="click->clipboard#copy">
export default class extends Controller {
  static values = { text: String }

  copy() {
    navigator.clipboard.writeText(this.textValue).catch((error) => {
      console.error("Clipboard copy failed", error)
    })
  }
}
