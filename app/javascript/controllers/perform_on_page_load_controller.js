import { Controller } from "@hotwired/stimulus";
import { createConsumer } from "@rails/actioncable";

// Connects to data-controller="perform-on-page-load"
export default class extends Controller {
  static values = { channel: String, method: String, methodArgs: Object };

  connect() {
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
