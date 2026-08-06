# frozen_string_literal: true

require 'test_helper'

class CspReportsControllerTest < ActionDispatch::IntegrationTest
  def report(body)
    post_body(body.to_json)
  end

  def post_body(raw)
    post '/csp-report', params: raw, headers: { 'CONTENT_TYPE' => 'application/csp-report' }
  end

  # Capture what actually reached the logger. Mocha replaces the singleton method on the
  # very object the controller calls; the block always matches, so nothing is filtered out
  # on the way in and the assertions can do the filtering instead.
  def violations
    lines = []
    Rails.logger.stubs(:warn).with { |message| lines << message.to_s }
    yield
    lines.grep(/\[csp-violation\]/)
  end

  # allow_forgery_protection is false in the test environment, so a test that only posts a
  # report and checks the status stays green with skip_forgery_protection deleted — and a
  # browser sends no CSRF token with a violation report, so in production that deletion
  # would 422 every one of them. Turning protection back on for the duration is what makes
  # this cover the production path. The log assertion is the second half: a controller that
  # did nothing but `head :no_content` would satisfy the status on its own.
  test 'accepts a report with no session and no CSRF token, and logs it' do
    ActionController::Base.allow_forgery_protection = true

    lines = violations do
      report('csp-report' => { 'blocked-uri' => 'inline', 'violated-directive' => 'script-src' })
    end

    assert_response :no_content
    assert_equal ['[csp-violation] violated-directive=script-src blocked-uri=inline'], lines
  ensure
    ActionController::Base.allow_forgery_protection = false
  end

  # Per the CSP reporting algorithm the browser strips the fragment and any credentials
  # from these URLs but keeps the query string, and Devise's reset link is
  # /password/edit?reset_password_token=… — so a violation fired on that page would write a
  # live password-reset token into the log.
  test 'never logs a password-reset token from document-uri' do
    lines = violations do
      report('csp-report' => {
               'document-uri' => 'https://x.test/password/edit?reset_password_token=SEKRIT',
               'violated-directive' => 'script-src'
             })
    end

    assert_equal 1, lines.size
    refute_includes lines.first, 'SEKRIT'
  end

  test 'keeps the path so the report is still useful' do
    lines = violations do
      report('csp-report' => { 'document-uri' => 'https://x.test/password/edit?reset_password_token=SEKRIT' })
    end

    assert_includes lines.first, '/password/edit'
  end

  # The endpoint is unauthenticated, so every value in the line below is text a stranger
  # chose. A lone CR is enough to make one logged report look like two log lines.
  test 'collapses control characters so a report cannot forge a log line' do
    lines = violations { report('csp-report' => { 'violated-directive' => "script-src\r\ninjected" }) }

    assert_equal 1, lines.size
    refute_match(/[[:cntrl:]]/, lines.first)
  end

  # The inverse of forging a line: poisoning the one that is written. A Rails log is read —
  # by a person skimming `docker logs` and by log scanners alike — as a stream in which an
  # uppercase-initial, optionally scope-resolved token names an exception class. Left
  # as-is, an unauthenticated caller could plant exceptions that never happened into
  # somebody's alerting, which is worse than noise: it is noise that looks like a fault.
  test 'a report cannot make the log line read as a Ruby exception' do
    lines = violations do
      report('csp-report' => {
               'blocked-uri' => 'https://evil.test/ActiveRecord::StatementInvalid',
               'violated-directive' => 'Net::ReadTimeout',
               'effective-directive' => 'SomeUnscopedError here'
             })
    end

    line = lines.first
    refute_match(/[A-Z][A-Za-z0-9_]*::/, line, 'a scope-resolved constant survived')
    refute_match(/[A-Z][A-Za-z]*(?:Error|Exception)/, line, 'an exception-shaped token survived')
    assert_includes line, 'statementinvalid', 'the value still has to be readable'
  end

  test 'one field cannot spend the whole log line' do
    lines = violations do
      report('csp-report' => { 'blocked-uri' => "https://evil.test/#{'a' * 3_000}",
                               'violated-directive' => 'script-src' })
    end

    blocked = lines.first[/blocked-uri=(\S*)/, 1]
    assert_operator blocked.length, :<=, CspReportsController::MAX_FIELD
    assert_includes lines.first, 'violated-directive=script-src',
                    'a long field must not push the other fields out'
  end

  # Both halves matter. Valid JSON is not necessarily a report — `null`, `[]` and
  # {"csp-report": []} all parse and would then blow up on Hash access — but a controller
  # that did nothing at all would also return 204 to every one of these, so the discards
  # only mean something standing next to a well-formed report that does get written.
  test 'a malformed or wrongly-shaped body is discarded quietly, but a real one is not' do
    ['not json', '', 'null', '[]', '"a string"', '{"csp-report":[]}', '{"csp-report":null}'].each do |body|
      lines = violations { post_body(body) }

      assert_response :no_content, "#{body.inspect} should be discarded quietly"
      assert_empty lines, "#{body.inspect} should not reach the log"
    end

    lines = violations { report('csp-report' => { 'blocked-uri' => 'inline' }) }
    assert_equal ['[csp-violation] blocked-uri=inline'], lines
  end

  # MAX_BODY is a read cap, so an oversized report truncates mid-JSON, fails to parse and
  # is dropped — and the biggest reports are the interesting ones. Dropping them is
  # defensible; dropping them with nothing written at all, in an endpoint whose entire job
  # is telemetry, is not.
  test 'an oversized report is refused visibly rather than dropped in silence' do
    padding = 'a' * (CspReportsController::MAX_BODY + 1_000)
    lines = violations { report('csp-report' => { 'blocked-uri' => padding }) }

    assert_response :no_content
    assert_equal 1, lines.size
    assert_match(/discarded/, lines.first)
  end

  # JSON.parse hands back a String tagged UTF-8 whose bytes need not be valid UTF-8: a lone
  # surrogate escape is legal JSON, and the body carrying one is pure ASCII, so nothing
  # upstream rejects it. Every blank? and every regex the controller runs then raises
  # ArgumentError. That is worse here than ordinary fragility — it is the inverse of the
  # neutralising above: a caller who cannot forge an exception through the sanitised path
  # would instead raise a real one, and each 500 writes a backtrace into the very log this
  # controller exists to keep tidy. Every logged field is probed, plus a key that is not
  # one, because any of them can carry it.
  test 'a field whose bytes are not valid UTF-8 does not raise' do
    logged = CspReportsController::SAFE_FIELDS + CspReportsController::URI_FIELDS

    (logged + ['not-a-field']).each do |field|
      lines = violations { post_body(%({"csp-report":{"#{field}":"x\\ud800y"}})) }

      assert_response :no_content, "an invalid byte sequence in #{field} must not raise"
      assert_equal(logged.include?(field) ? 1 : 0, lines.size,
                   "#{field} produced the wrong number of log lines")
      refute_match(/[[:cntrl:]]/, lines.first.to_s)
    end
  end

  # The other way in. A raw invalid byte in the body is not rejected either: JSON.parse
  # accepts it and hands back the same invalid-encoding String.
  test 'a raw invalid byte in the request body does not raise' do
    lines = violations do
      post_body(%({"csp-report":{"blocked-uri":"x\xFFy","violated-directive":"script-src"}}).b)
    end

    assert_response :no_content
    assert_equal 1, lines.size
    assert_includes lines.first, 'violated-directive=script-src'
  end

  # CR and LF are themselves control characters, so a test that injects only those and then
  # asserts no control character survived is equally satisfied by a scrub that knows about
  # nothing else. NUL, ESC, BEL and DEL are what tell the two apart — and an ESC sequence
  # in a logged value can rewrite the line of whoever is reading the log in a terminal.
  test 'control characters beyond CR and LF are stripped too' do
    lines = violations do
      report('csp-report' => { 'violated-directive' => "script-src \e[2Kwiped" })
    end

    assert_equal 1, lines.size
    refute_match(/[[:cntrl:]]/, lines.first)
    assert_includes lines.first, 'wiped', 'the value still has to be readable'
  end

  # A character cap is not a byte cap: one four-byte codepoint is a single character, so
  # nine fields capped by character reach four times the byte budget the cap reads as. The
  # cap is on bytes, and truncation has to land on a character boundary rather than
  # splitting one and producing the invalid encoding the test above is about.
  test 'the line is bounded in bytes, not just in characters' do
    lines = violations { report('csp-report' => { 'violated-directive' => '𝔘' * 800 }) }

    directive = lines.first[/violated-directive=(\S*)/, 1]
    assert_operator directive.bytesize, :<=, CspReportsController::MAX_FIELD
    assert directive.valid_encoding?, 'truncation must not split a multi-byte character'
  end
end
