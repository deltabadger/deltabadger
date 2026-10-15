import { Controller } from "@hotwired/stimulus";

// Width each bar had when Turbo replaced it, by container id, so its successor animates from there.
const lastWidths = new Map();

// Connects to data-controller="progress-bar"
export default class extends Controller {
  static values = {
    startTime: String,
    endTime: String,
  };
  connect() {
    this.key = this.element.parentElement?.id;
    this.#enter();

    const updateInterval = 500; // must match the transition-duration in the stylesheet
    if (this.startTimeValue && this.endTimeValue) {
      this.startTime = new Date(this.startTimeValue);
      this.endTime = new Date(this.endTimeValue) - updateInterval;
      this.interval = setInterval(() => this.#updateProgressBar(), updateInterval);
      this.#updateProgressBar();
    }
  }

  disconnect() {
    clearInterval(this.interval);
    if (this.key) lastWidths.set(this.key, this.element.style.width);
  }

  // Start from the replaced bar's width (0 on a fresh page) and move to the rendered one: a stopped
  // bar shrinks and fades out (ease-in), a bar coming from nothing grows and fades in (ease-out).
  #enter() {
    const width = this.element.style.width;
    const from = lastWidths.get(this.key) || "0%";
    const exiting = !this.startTimeValue;
    const style = this.element.style;
    style.transition = "none";
    style.width = from;
    style.opacity = exiting || parseFloat(from) > 0 ? 1 : 0;
    this.element.offsetWidth; // commit the start state before transitioning
    // Opacity runs in the first half on the way in, the second half on the way out.
    style.transition = exiting
      ? "width 0.25s ease-in, opacity 0.125s linear 0.125s"
      : "width 0.25s ease-out, opacity 0.125s linear";
    style.width = width;
    style.opacity = exiting ? 0 : 1;
    // Back to the stylesheet's linear ticking once done; a timer fires even when the width didn't change.
    setTimeout(() => (style.transition = ""), 250);
  }

  #updateProgressBar() {
    if (this.startTime < this.endTime) {
      let progressPercentage =
        (new Date() - this.startTime) / (this.endTime - this.startTime);
      progressPercentage = Math.min(Math.max(progressPercentage, 0), 1);
      this.element.style.width = `${progressPercentage * 100}%`;
    }
  }
}
