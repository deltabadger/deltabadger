import { Controller } from "@hotwired/stimulus"

// Disables the toggle checkbox after status change.
// Re-enabled when Turbo broadcast replaces the tile.
export default class extends Controller {
  static targets = ["checkbox"]

  disable() {
    this.checkboxTarget.disabled = true
  }
}
