import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="bot--barbell-allocation"
export default class extends Controller {
  static targets = ["allocation0", "allocation0Text", "allocation1Text", "allocation0Slider"];

  connect() {
  }

  disconnect() {
  }

  updateAllocation0(event) {
    const formattedInput = parseFloat(event.target.value.replace(',','.').trim())
    let newValue = 0;
    if (!isNaN(formattedInput)) {
      newValue = Math.min(1, Math.max(0, formattedInput));
    }
    this.allocation0Target.value = newValue;
    this.allocation0TextTarget.textContent = (newValue * 100).toFixed(0);
    this.allocation1TextTarget.textContent = ((1 - newValue) * 100).toFixed(0);
    this.allocation0SliderTarget.style.width = String(newValue * 100) + '%';
  }
}
