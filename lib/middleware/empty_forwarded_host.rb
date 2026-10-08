# frozen_string_literal: true

# An X-Forwarded-Host that names no host (`,`) makes ActionDispatch::HostAuthorization raise while it
# reads request.host (a 500). Inserted in front of it, this refuses such a request the way
# HostAuthorization refuses a blocked host: 403 with an empty body.
class EmptyForwardedHost
  def initialize(app)
    @app = app
  end

  def call(env)
    forwarded = env['HTTP_X_FORWARDED_HOST'].presence
    # The expression Rails reads the host with (ActionDispatch::Http::URL#raw_host_with_port).
    return @app.call(env) unless forwarded && forwarded.split(/,\s?/).last.nil?

    xhr = env['HTTP_X_REQUESTED_WITH'].to_s.downcase.include?('xmlhttprequest')
    [403, { 'content-type' => "#{xhr ? 'text/plain' : 'text/html'}; charset=UTF-8" }, []]
  end
end
