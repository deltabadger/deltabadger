import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="progress-bar"
export default class extends Controller {
  static values = {
    startTime: String,
    endTime: String,
  };
  static targets = ["progressBar"];

  connect() {
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
  }

  #updateProgressBar() {
    if (this.startTime < this.endTime) {
      let progressPercentage =
        (new Date() - this.startTime) / (this.endTime - this.startTime);
      progressPercentage = Math.min(Math.max(progressPercentage, 0), 1);
      this.progressBarTarget.style.width = `${progressPercentage * 100}%`;
    }
  }
}
