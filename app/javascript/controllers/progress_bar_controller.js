import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="progress-bar"
export default class extends Controller {
  static values = {
    startTime: String,
    endTime: String,
  };
  static targets = ["progressBar"];

  connect() {
    const fps = 60;
    const updateInterval = 1000 / fps;
    this.startTime = new Date(this.startTimeValue);
    this.endTime = new Date(this.endTimeValue);
    this.updateProgressBar();
    this.progressInterval = setInterval(
      () => this.updateProgressBar(),
      updateInterval
    );
  }

  updateProgressBar() {
    let progressPercentage =
      (new Date() - this.startTime) / (this.endTime - this.startTime);
    progressPercentage = Math.min(Math.max(progressPercentage, 0), 1);
    this.progressBarTarget.style.width = `${progressPercentage * 100}%`;
  }
}
