import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="twitter"
export default class extends Controller {
  connect() {
    if (!window.twttr) {
      const script = document.createElement("script");
      script.src = "https://platform.twitter.com/widgets.js";
      script.async = true;
      // script.onload = () => {
      //   // Ensure widgets are processed after script loads
      //   if (window.twttr && window.twttr.widgets) {
      //     window.twttr.widgets.load(this.element);
      //   }
      // };
      document.head.appendChild(script);
    // } else if (window.twttr.widgets) {
    //   // If script is already loaded, process widgets immediately
    //   window.twttr.widgets.load(this.element);
    }
  }
}
