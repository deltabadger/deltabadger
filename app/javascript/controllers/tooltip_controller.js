import { Controller } from "@hotwired/stimulus";

export default class extends Controller {
  static values = { fixed: Boolean };
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

    if (this.fixedValue) {
      this.#positionFixed(tooltip);
      return;
    }

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

  // Pin the tooltip to the viewport so it escapes any ancestor with overflow:hidden.
  // Positioned just below the trigger, horizontally clamped within the viewport.
  #positionFixed(tooltip) {
    tooltip.style.position = "fixed";
    tooltip.style.left = "0px";
    tooltip.style.top = "0px";
    tooltip.style.right = "auto";
    tooltip.style.bottom = "auto";

    const triggerRect = this.element.getBoundingClientRect();
    const tipRect = tooltip.getBoundingClientRect();
    const margin = 8;

    let left = triggerRect.left + (triggerRect.width - tipRect.width) / 2;
    if (left + tipRect.width > window.innerWidth - margin) {
      left = window.innerWidth - margin - tipRect.width;
    }
    if (left < margin) left = margin;

    const top = triggerRect.bottom + 6;

    tooltip.style.left = `${left}px`;
    tooltip.style.top = `${top}px`;
  }
}
