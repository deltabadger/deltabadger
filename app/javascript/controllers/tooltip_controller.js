import { Controller } from "@hotwired/stimulus";

export default class extends Controller {
  static dynamicDelay = 500;
  static hoverTimeout = null;
  static noHoverTimeout = null;

  connect() {
    this.showTooltipTimeout = null;
  }

  toggle() {
    this.element.classList.toggle("show-tooltip");
  }

  showTooltip() {
    this.#clearTimeouts();
    this.constructor.hoverTimeout = setTimeout(() => {
      this.constructor.dynamicDelay = 0;
    }, 500);

    this.#setTooltipTimeout();
  }

  hideTooltip() {
    this.#clearTimeouts();
    this.element.classList.remove("show-tooltip");

    this.constructor.noHoverTimeout = setTimeout(() => {
      this.constructor.dynamicDelay = 500;
    }, 500);
  }

  #clearTimeouts() {
    clearTimeout(this.showTooltipTimeout);
    clearTimeout(this.constructor.hoverTimeout);
    clearTimeout(this.constructor.noHoverTimeout);
  }

  #setTooltipTimeout() {
    this.showTooltipTimeout = setTimeout(() => {
      this.element.classList.add("show-tooltip");
      this.#adjustPosition();
    }, this.constructor.dynamicDelay);
  }

  #adjustPosition() {
    const tooltip = this.element.querySelector(".tooltip");
    if (!tooltip) return;

    tooltip.style.left = "";
    const rect = tooltip.getBoundingClientRect();
    const margin = 8;

    if (rect.right > window.innerWidth - margin) {
      const overflow = rect.right - window.innerWidth + margin;
      tooltip.style.left = `${-overflow}px`;
    }
    if (rect.left < margin) {
      tooltip.style.left = `${margin - rect.left + parseFloat(tooltip.style.left || 0)}px`;
    }
  }
}
