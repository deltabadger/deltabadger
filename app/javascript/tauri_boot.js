// Sets the desktop class while the head is still parsing, before the stylesheet
// applies, so the chrome never paints in its browser form first. The main bundle
// cannot do this: it is deferred, and its desktop module resolves several dynamic
// imports before it gets there.
if (window.__IS_TAURI__) document.documentElement.classList.add("tauri");
