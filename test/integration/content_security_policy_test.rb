# frozen_string_literal: true

require 'test_helper'

class ContentSecurityPolicyTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  # The whole header, in order. Pinning the string rather than a few substrings is the
  # point: a directive that quietly widens (a bare scheme-source added to connect-src, say)
  # still passes every "assert_includes" written against it.
  EXPECTED = "default-src 'self'; font-src 'self' data:; img-src 'self' data: https:; " \
             "object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'; " \
             "script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; " \
             "connect-src 'self' ipc: http://ipc.localhost; report-uri /csp-report"

  setup { @admin = create(:user, admin: true, setup_completed: true) }

  # Report-only is the shipped disposition and this test says so on purpose: with
  # script-src carrying 'unsafe-inline' the policy reports, it does not defend.
  test 'emits a report-only policy on an unauthenticated page' do
    get '/en/login'

    assert_response :success
    assert_nil response.headers['Content-Security-Policy'],
               'nothing is enforced yet; the header must be the report-only one'
    assert_equal EXPECTED, response.headers['Content-Security-Policy-Report-Only']
  end

  # Stands in for the manual click-through. The header comes from middleware, so what is
  # worth pinning is that it survives on the pages people actually sit on, including one
  # rendered by a controller that does not inherit ApplicationController (/up).
  test 'every representative page carries the same policy' do
    sign_in @admin

    ['/en/bots', '/en/tracker', '/en/settings/connect', '/health-check', '/up'].each do |path|
      get path

      assert_response :success, "#{path} did not render"
      assert_equal EXPECTED, response.headers['Content-Security-Policy-Report-Only'],
                   "#{path} carried no policy, or a different one"
    end
  end

  # A nonce-source or hash-source anywhere in a source list makes browsers ignore
  # 'unsafe-inline' (CSP3 "does element match source list", allow-all-inline), which would
  # take out every inline handler, inline <script> block and style="" attribute at once.
  # The commented-out initializer this replaces shipped the two nonce lines a keystroke
  # away, so the absence is pinned rather than assumed. csp_meta_tag renders now that a
  # policy exists; it must render empty.
  test 'no nonce is generated, so unsafe-inline keeps working' do
    get '/en/login'

    assert_response :success
    assert_nil Rails.application.config.content_security_policy_nonce_generator
    refute_includes response.headers['Content-Security-Policy-Report-Only'], 'nonce-'
    assert_select 'meta[name=?]', 'csp-nonce' do |tags|
      assert tags.all? { |tag| tag['content'].blank? },
             'a rendered nonce would make browsers ignore unsafe-inline'
    end
  end

  # 'self' already matches wss:// from an https page and ws:// from an http page on the
  # same host and port (CSP3 "does url match expression in origin", step 4.2), so
  # ActionCable's /cable needs nothing extra in any deployment. A bare wss: is a
  # scheme-source with no host constraint — it would permit a WebSocket to any host, on the
  # one directive that bounds where a page can send data. ws: is worse still: its
  # scheme-part matches http and https too.
  test 'connect-src does not permit a websocket to an arbitrary host' do
    get '/en/login'
    connect = response.headers['Content-Security-Policy-Report-Only'][/connect-src [^;]+/]

    assert_equal "connect-src 'self' ipc: http://ipc.localhost", connect
    refute_match(/\bwss?:/, connect, 'a bare websocket scheme-source has no host constraint')
  end

  # form-action is a navigation directive and inherits nothing: it is absent from CSP3's
  # "get fetch directive fallback list", so default-src does not cover it. Without it an
  # injected <form action="https://evil/"> can still post a page's fields off-origin, which
  # is precisely the hole 'unsafe-inline' on script-src leaves open.
  test 'form-action is set, since it does not fall back to default-src' do
    get '/en/login'

    assert_includes response.headers['Content-Security-Policy-Report-Only'], "form-action 'self'"
  end
end
