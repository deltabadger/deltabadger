import { Controller } from "@hotwired/stimulus"

// The drawer is held open by a turbo-permanent checkbox, so a Turbo visit carries the open state
// onto the page you just picked and the menu is still covering it. Picking a destination has to
// close it. A control the drawer OWNS must not — same rule, and the same marker, the dropdown menu
// follows: flipping a switch is not choosing where to go.
export default class extends Controller {
  close(event) {
    if (event.target.closest("[data-dropdown-keep-open]")) return;

    document.getElementById("hamburger").checked = false;
  }
}
