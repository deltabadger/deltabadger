import { Controller } from "@hotwired/stimulus";
import { createConsumer } from "@rails/actioncable";

// Connects to data-controller="perform-on-page-load"
export default class extends Controller {
  static values = { channel: String, method: String, methodArgs: Object };

  connect() {
    this.boundTriggerJob = this.#triggerJob.bind(this);
    this.hasTriggered = false;

    window.addEventListener("turbo:load", this.boundTriggerJob, { once: true });
    window.addEventListener("DOMContentLoaded", this.boundTriggerJob, { once: true });

    this.fallbackTimeout = setTimeout(() => {
      if (!this.hasTriggered) {
        console.log("Fallback: triggering job after 5 seconds");
        this.boundTriggerJob();
      }
    }, 5000);
  }

  disconnect() {
    if (this.boundTriggerJob) {
      window.removeEventListener("turbo:load", this.boundTriggerJob);
      window.removeEventListener("DOMContentLoaded", this.boundTriggerJob);
    }
    if (this.fallbackTimeout) {
      clearTimeout(this.fallbackTimeout);
    }
  }

  #triggerJob() {
    if (this.hasTriggered) return;

    this.hasTriggered = true;
    clearTimeout(this.fallbackTimeout);

    const consumer = createConsumer();
    const channel = this.channelValue;
    const method = this.methodValue;
    const methodArgs = this.methodArgsValue;

    consumer.subscriptions.create(
      { channel: channel },
      {
        connected() {
          console.log(`calling ${method}`);
          this.perform(method, methodArgs);
        },
        error(err) {
          console.error("Action Cable error:", err);
        },
      }
    );
  }
}