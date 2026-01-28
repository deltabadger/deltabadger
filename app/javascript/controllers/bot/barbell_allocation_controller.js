import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="bot--barbell-allocation"
export default class extends Controller {
  static values = {
    color0: String,
    color1: String,
  };
  static targets = [
    "allocation0",
    "allocation0Text",
    "allocation1Text",
    "allocation0SliderProgress",
    "allocation0SliderTrack",
    "allocation1SliderOthervalue",
    "allocation0BackgroundColor",
    "allocation1BackgroundColor",
    "allocation0Color",
    "allocation1Color",
  ];

  connect() {
    // Sync visual state with the current input value on connect
    // This ensures proper state after Turbo Stream replacements
    if (this.hasAllocation0Target) {
      const value = parseFloat(this.allocation0Target.value) || 0;
      this.#syncVisualState(value);
    }
  }

  disconnect() {}

  updateAllocation0(event) {
    const formattedInput = parseFloat(
      event.target.value.replace(",", ".").trim()
    );
    let newValue0 = 0;
    if (!isNaN(formattedInput)) {
      newValue0 = Math.min(1, Math.max(0, formattedInput));
    }
    this.allocation0Target.value = newValue0;
    this.#syncVisualState(newValue0);
  }

  #syncVisualState(newValue0) {
    const newValue1 = 1 - newValue0;

    // Update text displays
    if (this.hasAllocation0TextTarget) {
      this.allocation0TextTarget.textContent = (newValue0 * 100).toFixed(0);
    }
    if (this.hasAllocation1TextTarget) {
      this.allocation1TextTarget.textContent = (newValue1 * 100).toFixed(0);
    }

    // Update background colors
    if (this.hasAllocation0BackgroundColorTarget) {
      this.allocation0BackgroundColorTarget.style.backgroundColor =
        this.#transparentColor(this.color0Value, newValue0);
    }
    if (this.hasAllocation1BackgroundColorTarget) {
      this.allocation1BackgroundColorTarget.style.backgroundColor =
        this.#transparentColor(this.color1Value, newValue1);
    }

    // Update text colors
    if (this.hasAllocation0ColorTarget) {
      this.allocation0ColorTarget.style.color = this.#transparentColor(
        this.color0Value,
        newValue0
      );
    }
    if (this.hasAllocation1ColorTarget) {
      this.allocation1ColorTarget.style.color = this.#transparentColor(
        this.color1Value,
        newValue1
      );
    }

    // Update slider styles
    if (this.hasAllocation0SliderTrackTarget) {
      this.allocation0SliderTrackTarget.style.gridTemplateColumns =
        `${newValue0 * 100}% auto`;
      this.allocation0SliderTrackTarget.style.borderLeftColor =
        this.#transparentColor(this.color0Value, newValue0);
      this.allocation0SliderTrackTarget.style.borderRightColor =
        this.#transparentColor(this.color1Value, newValue1);
    }
    if (this.hasAllocation0SliderProgressTarget) {
      this.allocation0SliderProgressTarget.style.backgroundColor =
        this.#transparentColor(this.color0Value, newValue0);
    }
    if (this.hasAllocation1SliderOthervalueTarget) {
      this.allocation1SliderOthervalueTarget.style.backgroundColor =
        this.#transparentColor(this.color1Value, newValue1);
    }
  }

  #transparentColor(color, opacity, minOpacity = 0.6) {
    const adjustedOpacity = minOpacity + (1 - minOpacity) * opacity;
    const alpha = Math.round(adjustedOpacity * 255)
      .toString(16)
      .padStart(2, "0");
    return color + alpha;
  }
}
