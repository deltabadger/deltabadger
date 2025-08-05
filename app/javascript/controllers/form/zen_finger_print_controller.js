import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="form--zen-finger-print"
export default class extends Controller {
  static targets = ["fingerPrintId"]
  static values = { merchantTransactionId: String }

  connect() {
    console.log("Zen Fingerprint controller connected with Merchant Transaction ID:", this.merchantTransactionIdValue)
  }

  submitWithFingerPrintId(event) {
    event.preventDefault();
    console.log("Before submit triggered with Merchant Transaction ID:", this.merchantTransactionIdValue)

    var collectorData = getCollectorData(this.merchantTransactionIdValue);
    collectorData.then((fingerPrintId) => {
      console.log("Fingerprint ID generated:", fingerPrintId);

      this.fingerPrintIdTarget.value = fingerPrintId;
      console.log("Fingerprint ID added to form as hidden input");

      this.element.submit();
      console.log("Form submitted with fingerprint ID:", fingerPrintId)
    }).catch(error => {
      console.error("Error generating fingerprint:", error);
    });
  }
}