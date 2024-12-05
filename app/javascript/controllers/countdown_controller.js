import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="countdown"
export default class extends Controller {
  static targets = ["days", "hours", "minutes", "seconds"];
  static values = { targetDate: String }

  connect() {
    this.targetDate = new Date(this.targetDateValue);
    this.interval = setInterval(() => this.updateCountdown(), 1000);
    this.updateCountdown();
  }

  disconnect() {
    clearInterval(this.interval);
  }

  updateCountdown() {
    const now = new Date();
    const timeDiff = this.targetDate - now;

    // if (timeDiff <= 0) {
    //   this.element.innerHTML = `<div class="countdown-finished">Event has started!</div>`;
    //   clearInterval(this.interval);
    //   return;
    // }

    const days = Math.max(0, Math.floor(timeDiff / (1000 * 60 * 60 * 24)));
    const hours = Math.max(0, Math.floor((timeDiff % (1000 * 60 * 60 * 24)) / (1000 * 60 * 60)));
    const minutes = Math.max(0, Math.floor((timeDiff % (1000 * 60 * 60)) / (1000 * 60)));
    const seconds = Math.max(0, Math.floor((timeDiff % (1000 * 60)) / 1000));

    this.updateTarget(this.daysTarget, days);
    this.updateTarget(this.hoursTarget, hours);
    this.updateTarget(this.minutesTarget, minutes);
    this.updateTarget(this.secondsTarget, seconds);
  }

  updateTarget(target, value) {
    if (target.innerText !== value.toString()) {
      target.innerText = value;
      this.animateValue(target);
    }
  }

  animateValue(target) {
    target.classList.add("pulse");
    setTimeout(() => target.classList.remove("pulse"), 300); // Remove animation class after 300ms
  }
}