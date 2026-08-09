import "@hotwired/turbo-rails";
import "./controllers";
import "./tauri";

// Progressive web app support. All three layouts include this bundle, so the one
// registration covers every page; registration is idempotent.
if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/service-worker.js").catch((error) => {
    console.log("ServiceWorker registration failed: ", error);
  });
}
