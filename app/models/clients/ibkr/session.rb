require 'openssl'
require 'base64'

# Owns the IBKR Web API auth/session lifecycle for ONE api_key (= one IBKR login).
#
# - Mints the ~24h Diffie-Hellman "live session token" (LST) and caches it ENCRYPTED in
#   Solid Cache (shared across the web+jobs processes), re-minting on expiry.
# - Opens the brokerage session (ssodh/init) just-in-time, tracked by a short liveness flag.
# - Signs every request (HMAC-SHA256 keyed by the LST) and self-heals ONCE on an auth failure
#   (expired LST / dropped or competing brokerage session) by invalidating + re-establishing.
#
# Serialization across processes is the CALLER's responsibility (Clients::Ibkr / Exchanges::Ibkr
# wrap each op in IbkrLock); this class is lock-agnostic.
class Clients::Ibkr::Session < Client
  class AuthError < StandardError; end

  BASE_URL = 'https://api.ibkr.com'.freeze
  LST_PATH = '/v1/api/oauth/live_session_token'.freeze
  SSODH_INIT_PATH = '/v1/api/iserver/auth/ssodh/init'.freeze

  LST_TTL = 23.hours
  BROKERAGE_SESSION_TTL = 4.minutes

  def initialize(api_key:)
    super()
    @api_key = api_key
    dh = OpenSSL::PKey::DH.new(api_key.dh_param)
    @dh_prime = dh.p.to_s(16).downcase
    @dh_generator = dh.g.to_i
    @oauth = Clients::Ibkr::Oauth.new(
      consumer_key: api_key.key,
      access_token: api_key.access_token,
      access_token_secret: api_key.secret,
      signature_key: api_key.rsa_signature_key,
      encryption_key: api_key.rsa_encryption_key,
      dh_prime: @dh_prime,
      dh_generator: @dh_generator,
      realm: api_key.ibkr_realm.presence || 'limited_poa'
    )
  end

  # Returns the cached LST or mints a fresh one. Raises AuthError on a bad/unvalidatable response.
  def live_session_token(force: false)
    cached = read_cached_lst unless force
    cached || mint_live_session_token
  end

  # Performs a signed request to a protected /iserver|/portfolio endpoint, establishing the
  # brokerage session first and self-healing once on an auth failure. Returns the parsed body;
  # raises on failure (the Clients::Ibkr layer wraps this in with_rescue -> Result).
  def signed_request(method, path, query: {}, body: nil)
    attempts = 0
    begin
      attempts += 1
      ensure_brokerage_session
      signed_call(method, path, query: query, body: body)
    rescue Faraday::UnauthorizedError, AuthError
      raise if attempts > 1

      invalidate_session!
      retry
    end
  end

  def invalidate_session!
    Rails.cache.delete(lst_cache_key)
    Rails.cache.delete(brokerage_session_cache_key)
  end

  private

  # --- session establishment ---

  def ensure_brokerage_session
    return if Rails.cache.read(brokerage_session_cache_key)

    # compete: true takes over any existing brokerage session for this login (handles the
    # one-session-per-username constraint); publish: true opens the websocket-capable session.
    signed_call(:post, SSODH_INIT_PATH, body: { publish: true, compete: true })
    Rails.cache.write(brokerage_session_cache_key, true, expires_in: BROKERAGE_SESSION_TTL)
  end

  def mint_live_session_token
    pre = @oauth.prepend
    dh_random = Clients::Ibkr::Oauth.dh_random
    header = @oauth.live_session_token_header(
      url: full_url(LST_PATH), dh_challenge: @oauth.dh_challenge(dh_random), prepend: pre
    )
    body = request(:post, LST_PATH, header: header)
    dh_response = body.is_a?(Hash) ? body['diffie_hellman_response'] : nil
    raise AuthError, 'live_session_token: missing diffie_hellman_response' if dh_response.blank?

    lst = Clients::Ibkr::Oauth.calculate_live_session_token(
      dh_prime: @dh_prime, dh_random: dh_random, dh_response: dh_response, prepend: pre
    )
    unless Clients::Ibkr::Oauth.validate_live_session_token(
      lst, signature: body['live_session_token_signature'].to_s, consumer_key: @api_key.key
    )
      raise AuthError, 'live_session_token failed validation'
    end

    write_cached_lst(lst)
    lst
  end

  # --- HTTP ---

  def signed_call(method, path, query: {}, body: nil)
    header = @oauth.signed_header(
      method: method.to_s.upcase, url: full_url(path),
      live_session_token: live_session_token, query_params: stringify(query)
    )
    request(method, path, header: header, query: query, body: body)
  end

  def request(method, path, header:, query: {}, body: nil)
    response = connection.public_send(method) do |req|
      req.url path
      req.params = query if query.present?
      req.headers['Authorization'] = header
      req.body = body if body
    end
    response.body
  end

  def connection
    @connection ||= Faraday.new(url: BASE_URL, **OPTIONS) do |config|
      config.request :json
      config.response :json, content_type: /\bjson$/
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # --- LST cache (encrypted; Solid Cache is on-disk and backed up) ---

  def read_cached_lst
    blob = Rails.cache.read(lst_cache_key)
    blob && encryptor.decrypt_and_verify(blob)
  rescue ActiveSupport::MessageEncryptor::InvalidMessage
    nil
  end

  def write_cached_lst(token)
    Rails.cache.write(lst_cache_key, encryptor.encrypt_and_sign(token), expires_in: LST_TTL)
  end

  def encryptor
    key = Rails.application.key_generator.generate_key('ibkr/lst/v1', 32)
    ActiveSupport::MessageEncryptor.new(key)
  end

  def lst_cache_key
    "ibkr:lst:#{@api_key.id}"
  end

  def brokerage_session_cache_key
    "ibkr:bsession:#{@api_key.id}"
  end

  # --- helpers ---

  def full_url(path)
    "#{BASE_URL}#{path}"
  end

  def stringify(params)
    params.to_h.transform_keys(&:to_s).transform_values(&:to_s)
  end
end
