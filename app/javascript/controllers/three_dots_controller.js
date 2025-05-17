import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="three-dots"
export default class extends Controller {
  static targets = ["dots"];

  connect() {
    this.currentStage = 0;
    this.stages = ["", ".", ". .", ". . .", ". .", "."];
    this.#startAnimation();
  }

  disconnect() {
    this.#stopAnimation();
  }

  #startAnimation() {
    this.interval = setInterval(() => {
      this.currentStage = (this.currentStage + 1) % this.stages.length;
      this.dotsTarget.textContent = this.stages[this.currentStage];
    }, 1000); // Update every 500ms
  }

  #stopAnimation() {
    clearInterval(this.interval);
  }
}
