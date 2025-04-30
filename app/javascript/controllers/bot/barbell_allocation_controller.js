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
    "allocation0BackgroundColor",
    "allocation1BackgroundColor",
    "allocation0Color",
    "allocation1Color",
  ];

  connect() {}

  disconnect() {}

  updateAllocation0(event) {
    const formattedInput = parseFloat(
      event.target.value.replace(",", ".").trim()
    );
    let newValue0 = 0;
    if (!isNaN(formattedInput)) {
      newValue0 = Math.min(1, Math.max(0, formattedInput));
    }
    const newValue1 = 1 - newValue0;
    this.allocation0Target.value = newValue0;
    this.allocation0TextTarget.textContent = (newValue0 * 100).toFixed(0);
    this.allocation1TextTarget.textContent = (newValue1 * 100).toFixed(0);
    this.allocation0BackgroundColorTarget.style.backgroundColor =
      this.#transparentColor(this.color0Value, newValue0);
    this.allocation1BackgroundColorTarget.style.backgroundColor =
      this.#transparentColor(this.color1Value, newValue1);
    this.allocation0ColorTarget.style.color = this.#transparentColor(
      this.color0Value,
      newValue0
    );
    this.allocation1ColorTarget.style.color = this.#transparentColor(
      this.color1Value,
      newValue1
    );

    // style the slider
    this.allocation0SliderProgressTarget.style.width =
      String(newValue0 * 100) + "%";
    this.allocation0SliderProgressTarget.style.backgroundColor =
      this.#transparentColor(this.color0Value, newValue0);
    this.allocation0SliderTrackTarget.style.borderLeftColor =
      this.#transparentColor(this.color0Value, newValue0);
    this.allocation0SliderTrackTarget.style.backgroundColor =
      this.#transparentColor(this.color1Value, newValue1);
  }

  #transparentColor(color, opacity, minOpacity = 0.6) {
    const adjustedOpacity = minOpacity + (1 - minOpacity) * opacity;
    const alpha = Math.round(adjustedOpacity * 255)
      .toString(16)
      .padStart(2, "0");
    return color + alpha;
  }
}
