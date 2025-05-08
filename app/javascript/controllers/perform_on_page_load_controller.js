import { Controller } from "@hotwired/stimulus";
import { createConsumer } from "@rails/actioncable";

// Connects to data-controller="perform-on-page-load"
export default class extends Controller {
  static values = { channel: String, method: String, methodArgs: Object };

  connect() {
    // console.log("connected to perform-on-page-load controller");
    this.boundTriggerJob = this.#triggerJob.bind(this);
    window.addEventListener("turbo:load", this.boundTriggerJob, { once: true });
    window.addEventListener("DOMContentLoaded", this.boundTriggerJob, { once: true });
  }

  disconnect() {
    // console.log("disconnected from perform-on-page-load controller");
    if (this.boundTriggerJob) {
      // console.log("removing event listeners");
      window.removeEventListener("turbo:load", this.boundTriggerJob);
      window.removeEventListener("DOMContentLoaded", this.boundTriggerJob);
    }
  }

  #triggerJob() {
    // console.log("#triggerJob");
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
