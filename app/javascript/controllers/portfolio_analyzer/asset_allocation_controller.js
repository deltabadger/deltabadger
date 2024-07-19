import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="portfolio-analyzer--asset-allocation"
export default class extends Controller {
  static targets = ["allocation", "allocationInputText", "allocationSlider"];
  static values = { assetApiId: String };

  connect() {
    window.addEventListener('riskLevelUpdated', this.#handleRiskLevelUpdated);
  }

  disconnect() {
    window.removeEventListener('riskLevelUpdated', this.#handleRiskLevelUpdated);
  }

  updateAssetAllocation(event) {
    const isRangeInput = event.target.type === "range";
    const formattedInput = parseFloat(event.target.value.replace(',','.').trim())
    let newValue = 0;
    if (!isNaN(formattedInput)) {
      newValue = Math.min(1, Math.max(0, isRangeInput ? formattedInput : formattedInput / 100));
    }
    this.allocationTarget.value = newValue;
    this.allocationInputTextTarget.value = (newValue * 100).toFixed(2);
    this.allocationSliderTarget.style.width = String(newValue * 100) + '%';
  }

  #handleRiskLevelUpdated = (event) => {
    if (this.assetApiIdValue in event.detail) {
      this.allocationTarget.value = event.detail[this.assetApiIdValue];
      this.allocationInputTextTarget.value = (event.detail[this.assetApiIdValue] * 100).toFixed(2);
      this.allocationSliderTarget.style.width = String(event.detail[this.assetApiIdValue] * 100) + '%';
    }
  }
}
