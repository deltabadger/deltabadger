import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="allocations"
export default class extends Controller {
  static targets = ["allocation", "allocationInputText", "allocationSlider", "riskLevel"];
  static values = { assetTicker: String, riskLevels: Array, smartAllocations: Object };

  connect() {
    window.addEventListener('allocationsUpdated', this.handleAllocationsUpdated);
  }

  disconnect() {
    window.removeEventListener('allocationsUpdated', this.handleAllocationsUpdated);
  }

  updateAssetAllocation(event) {
    const isRangeInput = event.target.type === "range";
    const formattedInput = parseFloat(event.target.value.replace(',','.').trim())
    let newValue = 0;
    if (!isNaN(formattedInput)) {
      newValue = Math.min(1, Math.max(0, isRangeInput ? formattedInput : formattedInput / 100));
    }
    this.allocationTarget.value = newValue;
    this.allocationInputTextTarget.value = (newValue * 100).toFixed(0);
    this.allocationSliderTarget.style.width = String(newValue * 100) + '%';
  }

  updateRiskLevel(event) {
    this.riskLevelTarget.textContent = this.riskLevelsValue[event.target.value];
    const detail = this.smartAllocationsValue[event.target.value];
    const newEvent = new CustomEvent('allocationsUpdated', { detail });
    window.dispatchEvent(newEvent);
  }

  handleAllocationsUpdated = (event) => {
    if (this.assetTickerValue in event.detail) {
      this.allocationTarget.value = event.detail[this.assetTickerValue];
      this.allocationInputTextTarget.value = (event.detail[this.assetTickerValue] * 100).toFixed(0);
      this.allocationSliderTarget.style.width = String(event.detail[this.assetTickerValue] * 100) + '%';
    }
  }
}
