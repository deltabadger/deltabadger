# frozen_string_literal: true

# Be sure to restart your server when you modify this file.
#
# See the Securing Rails Applications Guide for more information:
# https://guides.rubyonrails.org/security.html#content-security-policy-header
#
# script-src no longer concedes 'unsafe-inline'. No view in this app renders script
# inline: handlers are Stimulus actions and blocks live in the bundle, which
# test/linting/inline_javascript_test.rb keeps true for every file under app/views.
#
# The nonce below exists for the inline <script> elements this app does not write.
# The jobs dashboard's import map is rendered by its engine, which already asks for
# a nonce and gets an empty one without a generator; Turbo stamps the same nonce on
# scripts it injects during stream and morph renders, reading it from csp_meta_tag.
# Neither can be moved into a bundle, and both are refused once the policy enforces.
#
# The nonce is confined to script-src. Rails defaults nonce_directives to script-src
# AND style-src, and a nonce-source anywhere in a source list makes browsers ignore
# 'unsafe-inline' in that same list (CSP3, "does element match source list for type
# and source": allow-all-inline returns "Does Not Allow" when a nonce is present).
# style-src keeps 'unsafe-inline' on purpose — style="" attributes are used all over
# the app and Turbo injects a <style> for its progress bar — so letting the nonce
# reach it would unstyle every page at once.
#
# Still report-only: the header below is final, and shipping it in this disposition
# is what lets real traffic report anything the inventory missed before it enforces.
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

    policy.script_src      :self
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
  #
  # Report-only, unconditionally. This release ships the final header and nothing
  # else; the whole value of the stage is that it cannot break a page, and a
  # disposition an environment variable can flip is not that. Enforcement is the
  # next release's single change.
  config.content_security_policy_report_only = true

  # A per-request nonce, for the inline <script> elements the app does not render
  # itself: the jobs dashboard's import map, and Turbo's own injected scripts,
  # both of which already look for one. Random per response rather than derived
  # from the session: a nonce that is constant for a session is guessable from any
  # page the holder can see, and putting the session identifier in the markup is
  # worse than the problem it solves. Safe against caching because no HTML
  # response here is cached.
  config.content_security_policy_nonce_generator = ->(_request) { SecureRandom.base64(16) }

  # script-src ONLY. Rails defaults this to %w[script-src style-src], and a
  # nonce-source anywhere in a source list makes browsers ignore 'unsafe-inline'
  # in that same list (CSP3, "does element match source list for type and
  # source": allow-all-inline returns "Does Not Allow" when a nonce is present).
  # style-src keeps 'unsafe-inline' because style="" attributes are used
  # throughout the app and Turbo injects a <style> for its progress bar, so a
  # nonce reaching this directive unstyles every page at once.
  config.content_security_policy_nonce_directives = %w[script-src]
end
