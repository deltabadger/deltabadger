import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="allocations"
export default class extends Controller {
  static targets = ["allocation", "allocationDisplay"];

  update(event) {
    const isRangeInput = event.target.type === "range";
    // const formattedInput = parseFloat(event.target.value.replace(',','.').replace('%', '').trim())
    const formattedInput = parseFloat(event.target.value.replace(',','.').trim())
    let newValue = 0;
    if (!isNaN(formattedInput)) {
      newValue = Math.min(1, Math.max(0, isRangeInput ? formattedInput : formattedInput / 100));
    }
    this.allocationTargets.forEach((target) => {
      if (target !== event.target) {
        target.value = newValue;
      }
    });
    this.allocationDisplayTargets.forEach((display) => {
      // display.value = String((newValue * 100).toFixed(2)) + " %";
      display.value = (newValue * 100).toFixed(0);
    });
  }
}
