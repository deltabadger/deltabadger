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
    }, this.constructor.dynamicDelay);
  }
}
