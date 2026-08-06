# frozen_string_literal: true

# Be sure to restart your server when you modify this file.
#
# See the Securing Rails Applications Guide for more information:
# https://guides.rubyonrails.org/security.html#content-security-policy-header
#
# THIS POLICY DOES NOT YET DEFEND AGAINST INJECTED SCRIPT. It ships report-only, and
# script-src still carries 'unsafe-inline' to keep the app's inline event handlers and
# inline <script> blocks working. That combination reports; it enforces nothing. It is here
# to measure real traffic and size the work, and it becomes protection only once the inline
# handlers have moved into the bundle, 'unsafe-inline' is gone from script-src, and
# CSP_ENFORCE=true is set.
#
# Do NOT restore config.content_security_policy_nonce_generator, which the commented-out
# default this replaced carried two lines below the policy. A nonce-source or hash-source
# anywhere in a source list makes browsers ignore 'unsafe-inline' (CSP3, "does element match
# source list for type and source": allow-all-inline returns "Does Not Allow" when a nonce
# or hash is present), so adding one would break every inline handler, every inline <script>
# and every style="" attribute in the app at once, plus the <style> Turbo injects for its
# progress bar on each navigation. Enabling the policy does make csp_meta_tag start
# rendering in the three layouts; with no generator it renders an empty tag, which Turbo
# reads as "no nonce" and ignores.
Rails.application.configure do
  config.content_security_policy do |policy|
    policy.default_src     :self
    policy.font_src        :self, :data
    policy.img_src         :self, :data, :https
    policy.object_src      :none
    policy.base_uri        :self
    policy.frame_ancestors :none

    # form-action is a navigation directive: it is absent from CSP3's "get fetch directive
    # fallback list", so it inherits nothing from default-src and form submissions are
    # otherwise unrestricted. No view in this app posts to an external host. With
    # 'unsafe-inline' conceded on script-src, this is one of the few directives here that
    # still closes something — an injected <form action="https://evil/">.
    policy.form_action     :self

    policy.script_src      :self, :unsafe_inline
    policy.style_src       :self, :unsafe_inline

    # No wss: or ws: on purpose. 'self' already matches wss:// from an https page and ws://
    # from an http page on the same host and port (CSP3, "does url match expression in
    # origin with redirect count", step 4.2), so ActionCable's /cable is covered in every
    # deployment shape this app has. A bare `wss:` is a scheme-source and carries no host
    # constraint: it would permit a WebSocket to any host on the internet, on the one
    # directive that bounds where a page can send data. `ws:` is worse — its scheme-part
    # also matches http and https. If a browser turns out not to implement that 'self'
    # rule, report-only is what will show it, and the fix is the specific origin.
    #
    # ipc: and http://ipc.localhost are Tauri v2's IPC transports. This header governs the
    # desktop webview too: src-tauri/tauri.conf.json sets "csp": null, so Tauri injects no
    # policy of its own, and src-tauri/src/lib.rs loads the Rails app itself at
    # http://127.0.0.1:PORT. Without these, every invoke() — opening an external link, the
    # CSV export save dialog, dragging the titlebar — is reported, and breaks outright once
    # the policy is enforced. `ipc:` is a scheme-source with no host constraint, which is
    # tolerable only because it is a webview-internal custom protocol rather than a way
    # onto the network; http://ipc.localhost is bound to that host.
    policy.connect_src     :self, 'ipc:', 'http://ipc.localhost'

    # Relative on purpose: hosted installs each live on their own subdomain, so a report
    # goes to the origin the page came from. The CSP grammar takes a uri-reference here.
    policy.report_uri      '/csp-report'
  end

  # frame-src is deliberately absent. The app embeds no iframes, so it would guard nothing
  # today, and leaving default-src to catch a future embed is what surfaces one in the
  # reports rather than letting it through unnoticed.
  config.content_security_policy_report_only = ENV['CSP_ENFORCE'] != 'true'
end
