import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="nested-link-in-button"
// This controller is used to allow a link to be clickable inside a button.
// Such links are clickable if the link content is a text but not if the content is a svg.
// In this case this controller is needed. Just add to the link_to element:
// data: {
//   controller: 'nested-link-in-button',
//   action: 'click->nested-link-in-button#click',
//   nested_link_in_button_target: "link_to"
// }
export default class extends Controller {
  static targets = ["link_to"];

  click(event) {
    event.preventDefault();
    window.open(this.link_toTarget.href, this.link_toTarget.target || '_self', this.link_toTarget.rel || '');
  }
}
