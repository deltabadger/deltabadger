# frozen_string_literal: true

require 'test_helper'

# script-src carries no 'unsafe-inline', so an inline event-handler attribute or
# an inline <script> in a view is dead markup: the browser refuses to compile it.
# This reads the view sources rather than a rendered page, so a view that no test
# renders is covered too, and a handler added in any locale or partial is caught
# at the point it is written.
class InlineJavascriptTest < ActiveSupport::TestCase
  VIEWS = Rails.root.join('app/views')

  # Every shape that reaches the browser as an inline handler:
  #   onclick="…"  onclick=…  onClick="…"   HTML attributes, quoted or not, any case
  #   onclick: "…"  onclick: some_var       the Rails helper hash form
  #   "onclick" => "…"                      the same, with a string key
  # The hash forms matter most: they are invisible to a search for `on\w+=`, and
  # that is exactly how one of these was missed when this work was scoped.
  HANDLER = /(?<![a-z])(on[a-z]+)["']?\s*(?:=>?|:)/i

  # `on[a-z]+` cannot tell an event name from a Ruby option that happens to start
  # with "on", so the handful that do are named rather than pattern-matched around.
  # `only_path:` is not among them: `[a-z]+` stops at the underscore, so the colon
  # is never adjacent.
  IGNORED_NAMES = %w[only once one].freeze

  # A literal block in ERB or HAML, the HAML filter, and the helper that builds one.
  SCRIPT = /<script\b|%script|:javascript\b|javascript_tag/i

  # A javascript: URL is inline script wearing a URL, and script-src refuses it on
  # the same grounds. Attribute forms only — quoted, unquoted, HAML hash and string
  # key. A bare `link_to "x", "javascript:…"` is not caught: matching any quoted
  # string that opens with `javascript:` would also fire on ordinary prose, and a
  # lint that cries wolf gets switched off. The policy is the defence here; this is
  # the early warning.
  JAVASCRIPT_URL = /(?:href|src|action)["']?\s*(?:=>|[:=])\s*["']?\s*javascript:/i

  # Views still carrying inline JavaScript. This list only ever shrinks.
  REMAINING = [
    'bots/dca_indexes/pick_indices/new.html.erb',
    'layouts/application.html.erb',
    'layouts/devise.html.erb',
    'layouts/guest.html.erb'
  ].freeze

  test 'no view carries inline JavaScript beyond the recorded remainder' do
    assert_equal REMAINING, offending_views,
                 'a view gained inline JavaScript, or a listed one no longer has any'
  end

  private

  def offending_views
    Dir.glob('**/*', base: VIEWS)
       .select { |path| File.file?(VIEWS.join(path)) }
       .select { |path| inline_javascript?(VIEWS.join(path)) }
       .sort
  end

  def inline_javascript?(path)
    source = File.read(path)

    inline_handler?(source) || source.match?(SCRIPT) || source.match?(JAVASCRIPT_URL)
  end

  def inline_handler?(source)
    source.scan(HANDLER).flatten.any? { |name| IGNORED_NAMES.exclude?(name.downcase) }
  end
end
