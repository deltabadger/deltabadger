import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="risk-level-allocations"
export default class extends Controller {
  static targets = ["riskLevelSlider"];
  static values = { riskLevels: Array, smartAllocations: Array };

  updateRiskLevel(event) {
    this.riskLevelSliderTarget.style.width = String(event.target.value / (this.riskLevelsValue.length - 1) * 100) + '%';
    const detail = this.smartAllocationsValue[event.target.value];
    const newEvent = new CustomEvent('riskLevelUpdated', { detail });
    window.dispatchEvent(newEvent);
  }
}
