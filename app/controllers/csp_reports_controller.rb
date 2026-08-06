# frozen_string_literal: true

# Browsers POST Content-Security-Policy violations here as application/csp-report. It takes
# no session and no CSRF token, because a browser sends neither with a report — so anyone at
# all can reach it, and everything it writes is text a stranger chose. All it does is log a
# fixed set of fields, each of them bounded and neutralised first.
#
# ActionController::Base rather than ApplicationController is required, not just tidy:
# ApplicationController's redirect_to_setup_if_needed would 302 every report to /setup until
# an admin exists, and switch_locale would touch the database once per report.
# HealthCheckController and Oauth::DynamicRegistrationController inherit it for the same
# reason.
class CspReportsController < ActionController::Base
  skip_forgery_protection

  # Read cap on the request body. Anything larger is refused and said so, rather than read
  # to the cap and left to fail as truncated JSON — real reports do exceed this (long
  # blocked-uris, deep source-file paths), and those are the interesting ones, so they must
  # not disappear without a trace.
  MAX_BODY = 4_000
  # Per-value cap, applied before the values are joined, so one field cannot spend the whole
  # line and push the rest out. The line is bounded by construction: a fixed set of nine
  # field names, each value at most this long.
  MAX_FIELD = 256

  # Never log the whole report body. Per the CSP reporting algorithm the browser strips the
  # fragment and any credentials from these URLs but KEEPS THE QUERY STRING — and Devise's
  # reset link is /password/edit?reset_password_token=…, so a violation fired on that page
  # would otherwise write a live password-reset token into the log.
  URI_FIELDS = %w[document-uri blocked-uri source-file referrer].freeze
  SAFE_FIELDS = %w[violated-directive effective-directive disposition status-code line-number].freeze

  # A Rails log is read — by a person skimming `docker logs` and by log scanners alike — as
  # a stream of lines in which an uppercase-initial, optionally scope-resolved token names
  # an exception class. Every value logged below is chosen by whoever POSTed the report, so
  # one shaped like `Foo::BarError` would let an unauthenticated caller invent faults in
  # someone else's alerting. Downcasing that shape keeps the text readable while removing
  # its ability to read as a Ruby constant. It does not make the value trustworthy — nothing
  # here can — it makes it unable to impersonate one.
  CONSTANT_SHAPE = /[A-Z][A-Za-z0-9_]*(?:::[A-Za-z0-9_]+)*/

  def create
    report = parsed_report
    return head :no_content if report.blank?

    fields = SAFE_FIELDS.index_with { |field| report[field] }
                        .merge(URI_FIELDS.index_with { |field| strip_query(report[field]) })
                        .compact_blank
                        .transform_values { |value| sanitize(value) }

    Rails.logger.warn("[csp-violation] #{fields.map { |name, value| "#{name}=#{value}" }.join(' ')}")
    head :no_content
  end

  private

  # Valid JSON is not necessarily a report: `null`, `[]` and {"csp-report": []} all parse
  # fine and would then blow up on Hash access.
  def parsed_report
    raw = request.body.read(MAX_BODY + 1).to_s
    return log_discarded("body over #{MAX_BODY} bytes") if raw.bytesize > MAX_BODY

    body = JSON.parse(raw)
    return nil unless body.is_a?(Hash)

    report = body['csp-report']
    report.is_a?(Hash) ? report : nil
  rescue JSON::ParserError
    nil
  end

  def log_discarded(reason)
    Rails.logger.warn("[csp-violation] discarded: #{reason}")
    nil
  end

  # Keep scheme, host and path; drop the query and fragment entirely.
  def strip_query(value)
    return nil if value.blank?

    uri = URI.parse(value.to_s)
    uri.query = nil
    uri.fragment = nil
    uri.to_s
  rescue URI::Error
    '[unparseable]'
  end

  # Control characters go first: the log is read line by line, so a lone CR in a report
  # would be enough to make one logged violation look like two lines, or to overwrite the
  # one being written.
  def sanitize(value)
    value.to_s
         .gsub(/[[:cntrl:]]+/, ' ')
         .gsub(CONSTANT_SHAPE, &:downcase)
         .truncate(MAX_FIELD)
  end
end
