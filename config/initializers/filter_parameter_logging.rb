# Be sure to restart your server when you modify this file.

# Configure sensitive parameters which will be filtered from the log file.
Rails.application.config.filter_parameters += %i[password key secret passphrase otp_code_token code token]

# The request line — `Started POST "/hook/…"` — is written from filtered_path, which masks the query
# string and nothing else: a path segment is logged verbatim. A signal bot's webhook token is a
# bearer secret in the path, so it is masked here, on the request itself, before any log line is
# built from it.
module HookTokenFilter
  HOOK_TOKEN = %r{\A(/hook/)[^/?]+}

  def filtered_path
    super.sub(HOOK_TOKEN, '\1[FILTERED]')
  end
end

ActionDispatch::Request.prepend(HookTokenFilter)
