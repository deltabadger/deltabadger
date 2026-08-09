# frozen_string_literal: true

require 'test_helper'

class ContentSecurityPolicyTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup { @admin = create(:user, admin: true, setup_completed: true) }

  # The whole header, in order, with only the per-request nonce factored out.
  # Pinning the string rather than a few substrings is the point: a directive
  # that quietly widens still passes every "assert_includes" written against it.
  def expected_policy(nonce)
    "default-src 'self'; font-src 'self' data:; img-src 'self' data: https:; " \
      "object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'; " \
      "script-src 'self' 'nonce-#{nonce}'; style-src 'self' 'unsafe-inline'; " \
      "connect-src 'self' ipc: http://ipc.localhost; report-uri /csp-report"
  end

  def policy_header
    response.headers['Content-Security-Policy-Report-Only']
  end

  def meta_nonce
    css_select('meta[name="csp-nonce"]').first&.[]('content')
  end

  # The header's nonce and the meta tag's must agree: Turbo reads the meta tag to
  # stamp scripts it injects during stream and morph renders, so a mismatch means
  # those scripts are refused once the policy is enforced.
  test 'the policy matches, and its nonce is the one the page advertises' do
    get '/en/login'

    assert_response :success
    assert_nil response.headers['Content-Security-Policy'],
               'nothing is enforced yet; the header must be the report-only one'
    assert_equal expected_policy(meta_nonce), policy_header
  end

  # Stands in for the manual click-through. The header comes from middleware, so what is
  # worth pinning is that it survives on the pages people actually sit on, including one
  # rendered by a controller that does not inherit ApplicationController (/up).
  #
  # These render a layout, so the meta tag is available and must agree with the
  # header — comparing the header against a nonce parsed out of itself is circular.
  # These render no layout and so advertise no meta nonce; the header is the only
  # thing there is to read.
  test 'every representative page carries the same policy' do
    sign_in @admin

    ['/en/bots', '/en/tracker', '/en/settings/connect'].each do |path|
      get path

      assert_response :success, "#{path} did not render"
      assert_equal expected_policy(meta_nonce), policy_header, "#{path} carried a different policy"
    end

    ['/health-check', '/up'].each do |path|
      get path

      assert_response :success, "#{path} did not render"
      nonce = policy_header[/'nonce-([^']+)'/, 1]

      assert_equal expected_policy(nonce), policy_header, "#{path} carried a different policy"
    end
  end

  # style-src must never take a nonce. A nonce-source anywhere in a source list
  # makes browsers ignore 'unsafe-inline' in that list, and this app styles with
  # style="" attributes throughout, so a nonce here unstyles the whole app at once.
  # Rails' default nonce_directives includes style-src, which is why this is pinned.
  test 'style-src carries no nonce' do
    get '/en/login'

    assert_equal %w[script-src], Rails.application.config.content_security_policy_nonce_directives
    assert_equal "style-src 'self' 'unsafe-inline'", policy_header[/style-src [^;]+/]
    refute_match(/style-src[^;]*nonce-/, policy_header)
  end

  test 'script-src no longer concedes unsafe-inline' do
    get '/en/login'

    refute_match(/script-src[^;]*'unsafe-inline'/, policy_header)
  end

  test 'each response gets its own nonce' do
    get '/en/login'
    first = meta_nonce
    get '/en/login'

    assert_not_equal first, meta_nonce, 'a reused nonce is no better than unsafe-inline'
  end

  # 'self' already matches wss:// from an https page and ws:// from an http page on the
  # same host and port (CSP3 "does url match expression in origin", step 4.2), so
  # ActionCable's /cable needs nothing extra in any deployment. A bare wss: is a
  # scheme-source with no host constraint — it would permit a WebSocket to any host, on the
  # one directive that bounds where a page can send data. ws: is worse still: its
  # scheme-part matches http and https too.
  test 'connect-src does not permit a websocket to an arbitrary host' do
    get '/en/login'
    connect = policy_header[/connect-src [^;]+/]

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

  test 'the signed-in layout ships no inline script block' do
    sign_in @admin
    get '/en/bots'

    assert_response :success
    assert_select 'script:not([src])', false, 'the application layout still has an inline block'
  end

  test 'the signed-out layout ships no inline script block' do
    get '/en/login'

    assert_response :success
    assert_select 'script:not([src])', false, 'the devise layout still has an inline block'
  end

  test 'the desktop class is set by a script the policy allows' do
    get '/en/login'

    assert_select 'script[src*=?]', 'tauri_boot'
  end

  # The source scanner in test/linting covers app/views. This covers what gems and
  # helpers render, which no source scan can see: every script that reaches the
  # browser is external or nonced, and no element arrives with an on* attribute.
  test 'rendered pages carry no inline handler and no unnonced script' do
    # /en/login is requested first, before signing in: Devise redirects a signed-in
    # user away from it, and a redirect body would satisfy every assertion below
    # while inspecting nothing.
    assert_clean_of_inline_javascript('/en/login')

    sign_in @admin
    ['/en/bots', '/en/settings/connect', '/jobs'].each do |path|
      assert_clean_of_inline_javascript(path)
    end
  end

  def assert_clean_of_inline_javascript(path)
    get path

    assert_response :success, "#{path} did not render, so it proved nothing"

    scripts = css_select('script')

    assert_predicate scripts, :any?, "#{path} rendered no script at all — check the path"
    scripts.each do |script|
      assert script['src'].present? || script['nonce'].present?,
             "#{path} rendered an inline script with no nonce"
    end

    # Attribute names rather than a regex over the body: a body match would fire on
    # any text that happens to read like one, inside a URL or a data attribute.
    handlers = css_select('*').flat_map { |node| node.attribute_nodes.map(&:name) }
                              .select { |name| name.downcase.start_with?('on') }.uniq

    assert_empty handlers, "#{path} rendered inline handler attributes"
  end

  # The dashboard whose inline script the nonce exists for. Asserting the tags are
  # nonced is not enough on its own: a nonce that does not match the header's is
  # refused just as a missing one is, and that mismatch is exactly what a
  # misconfigured generator produces.
  test 'the jobs dashboard renders importmap tags carrying this response nonce' do
    sign_in @admin
    get '/jobs'

    assert_response :success
    nonce = policy_header[/'nonce-([^']+)'/, 1]
    importmap = css_select('script[type=importmap]')

    assert_predicate importmap, :any?, 'the import map did not render'
    assert_equal [nonce], importmap.map { |tag| tag['nonce'] }.uniq
    assert_equal [nonce], css_select('script[type=module]:not([src])').map { |tag| tag['nonce'] }.uniq
  end
end
