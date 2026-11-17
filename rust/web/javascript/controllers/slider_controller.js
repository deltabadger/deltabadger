import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="slider"
// Generic slider controller that updates visual progress while dragging
export default class extends Controller {
  static targets = ["input", "track", "value"];
  static values = {
    suffix: { type: String, default: "" },
  };

  connect() {
    this.update();
  }

  update(event) {
    const input = this.hasInputTarget ? this.inputTarget : this.element.querySelector('input[type="range"]');
    if (!input) return;

    const min = parseFloat(input.min) || 0;
    const max = parseFloat(input.max) || 100;
    const value = parseFloat(input.value) || 0;

    // Calculate percentage for progress bar
    const percentage = ((value - min) / (max - min)) * 100;

    // Update track grid
    const track = this.hasTrackTarget ? this.trackTarget : this.element.querySelector('.slider__style__track');
    if (track) {
      track.style.gridTemplateColumns = `${percentage}% auto`;
    }

    // Update value display if target exists
    if (this.hasValueTarget) {
      let displayValue = value;
      // If max is 1 (like allocation_flattening), display as percentage
      if (max <= 1) {
        displayValue = Math.round(value * 100);
      }
      this.valueTarget.textContent = `${displayValue}${this.suffixValue}`;
    }
  }
}
