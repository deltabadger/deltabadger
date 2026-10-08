module LocaleHelper
  # Path-only URL for the current page with the locale swapped.
  #
  # The routing options come ONLY from the recognized route (request.path_parameters).
  # The query string is passed through url_for's dedicated `params:` option, which
  # emits every key verbatim into the query string and never interprets one as a
  # routing option. That distinction is the whole fix: merging the query into the
  # options hash let `?host=evil.com` rewrite the link's origin, `?protocol=javascript`
  # produce a `javascript:` href, and `?controller=admin` raise UrlGenerationError.
  #
  # Do not "simplify" this back into a single merge.
  #
  # The reserved-key filter is belt-and-braces: `params:` alone is already safe (those
  # keys would emit as literal query text), but there is no reason to carry an
  # attacker's `host=evil.com` along in every language link.
  #
  # `locale` is reserved too, for a different reason: this helper always supplies
  # `locale:` itself as a routing option below, so a query-string `locale` is by
  # definition stale. On a locale-scoped route (e.g. /login) that stale value is
  # merely harmless cruft — the path segment already consumed the routing option.
  # But on a route with no :locale segment (e.g. /setup), url_for's Journey leaves
  # the routing option {:locale => "de"} unconsumed, so it survives alongside the
  # query's {"locale" => "fr"} — different key types, so they don't collide and get
  # merged, they both land in the query string as `locale=de&locale=fr`. Rack's
  # last-wins query parsing then renders whichever sorts later, not whichever the
  # user actually clicked, silently pinning the page to a stale locale.
  URL_FOR_RESERVED = %w[
    host protocol port script_name anchor only_path trailing_slash
    subdomain domain tld_length params relative_url_root controller action format locale
  ].freeze

  # Credential parameters never ride along into a language link, at any nesting depth (Rust's
  # locale::sensitive_query holds the same list; a nested key is dropped when any part matches).
  SENSITIVE_QUERY = %w[
    confirmation_token password password_confirmation current_password otp_secret_key otp_secret
    otp_code_token key secret passphrase access_token refresh_token rsa_signature_key
    rsa_encryption_key dh_param api_token smtp_username smtp_password coingecko_api_key
    alpaca_api_key alpaca_api_secret market_data_token reset_password_token unlock_token token
    claim_code code
  ].freeze

  def locale_switch_path(locale)
    url_for(
      request.path_parameters.merge(
        locale: locale,
        only_path: true,
        params: without_sensitive_query(request.query_parameters.except(*URL_FOR_RESERVED))
      )
    )
  end

  private

  def without_sensitive_query(value)
    case value
    when Hash then value.each_with_object({}) do |(key, inner), kept|
      kept[key] = without_sensitive_query(inner) unless SENSITIVE_QUERY.include?(key.to_s.downcase(:ascii))
    end
    when Array then value.map { |inner| without_sensitive_query(inner) }
    else value
    end
  end
end
