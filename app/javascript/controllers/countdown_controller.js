import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="countdown"
export default class extends Controller {
  static targets = ["days", "hours", "minutes", "seconds", "daysLabel", "hoursLabel", "minutesLabel", "secondsLabel"];
  static values = { endTime: String, animationClass: String, hideIfZero: Boolean, daysLabel: String, hoursLabel: String, minutesLabel: String, secondsLabel: String }

  connect() {
    this.endTime = this.endTimeValue ? new Date(this.endTimeValue) : new Date();
    this.interval = setInterval(() => this.#updateCountdown(), 1000);
    this.#updateCountdown();
  }

  disconnect() {
    clearInterval(this.interval);
  }

  #updateCountdown() {
    const now = new Date();
    const timeDiff = this.endTime - now;

    // if (timeDiff <= 0) {
    //   this.element.innerHTML = `<div class="countdown-finished">Event has started!</div>`;
    //   clearInterval(this.interval);
    //   return;
    // }

    const days = Math.max(0, Math.floor(timeDiff / (1000 * 60 * 60 * 24)));
    const hours = Math.max(0, Math.floor((timeDiff % (1000 * 60 * 60 * 24)) / (1000 * 60 * 60)));
    const minutes = Math.max(0, Math.floor((timeDiff % (1000 * 60 * 60)) / (1000 * 60)));
    const seconds = Math.max(0, Math.floor((timeDiff % (1000 * 60)) / 1000));

    this.updateTarget(this.daysTarget, `${days}`);
    this.daysLabelTarget.innerText = this.daysLabelValue;
    this.updateTarget(this.hoursTarget, `${hours}`);
    this.hoursLabelTarget.innerText = this.hoursLabelValue;
    this.updateTarget(this.minutesTarget, `${minutes}`);
    this.minutesLabelTarget.innerText = this.minutesLabelValue;
    this.updateTarget(this.secondsTarget, `${seconds}`);
    this.secondsLabelTarget.innerText = this.secondsLabelValue;

    if (this.hideIfZeroValue) {
      if (days === 0) {
        this.daysTarget.classList.add("hidden");
        this.daysLabelTarget.classList.add("hidden");
      }
      if (hours === 0 && days === 0) {
        this.hoursTarget.classList.add("hidden");
        this.hoursLabelTarget.classList.add("hidden");
      }
      if (minutes === 0 && hours === 0 && days === 0) {
        this.minutesTarget.classList.add("hidden");
        this.minutesLabelTarget.classList.add("hidden");
      }
      // if (seconds === 0 && minutes === 0 && hours === 0 && days === 0) {
      //   this.secondsTarget.classList.add("hidden");
      //   this.secondsLabelTarget.classList.add("hidden");
      // }
    }
  }

  updateTarget(target, value) {
    if (target.innerText !== value.toString()) {
      target.innerText = value;
      this.animateValue(target);
    }
  }

  animateValue(target) {
    if (this.animationClassValue) {
      target.classList.add(this.animationClassValue);
      setTimeout(() => target.classList.remove(this.animationClassValue), 300); // Remove animation class after 300ms
    }
  }
}