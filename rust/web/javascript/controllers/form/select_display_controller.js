import { Controller } from "@hotwired/stimulus"

// Updates visible label text when a <select> value changes.
// Used with the sinput--select pattern where the select is invisible
// and a label span shows the selected option text.
//
// Connects to data-controller="form--select-display"
export default class extends Controller {
  static targets = ["label"]

  update(event) {
    const select = event.target
    this.labelTarget.textContent = select.options[select.selectedIndex].text
  }
}
