require 'openssl'
require 'base64'
require 'cgi'
require 'securerandom'

# IBKR Web API OAuth 1.0a signing (first-party / self-service).
#
# Gateway-free auth for https://api.ibkr.com: sign requests with RSA-SHA256 to mint a
# ~24h Diffie-Hellman "live session token" (LST), then HMAC-SHA256 each subsequent request
# with that token. Ported from IBKR's reference implementation (ibind oauth1a.py, "based on
# code provided directly by IBKR"); behaviour is pinned by test/models/clients/ibkr/oauth_test.rb
# against vectors computed independently in Python.
#
# Byte-exact details that break signatures if wrong, and are therefore unit-tested:
#   - #escape must match Python urllib quote_plus (space->"+", "~" stays, "/"->"%2F").
#   - .to_byte_array zero-pads when the integer's bit length is a multiple of 8.
class Clients::Ibkr::Oauth
  # Unreserved set per RFC3986 / Python quote_plus "always safe" chars; space handled separately.
  UNRESERVED = /[^A-Za-z0-9\-._~ ]/.freeze

  def initialize(consumer_key:, access_token:, access_token_secret:, signature_key:, encryption_key:,
                 dh_prime:, dh_generator: 2, realm: 'limited_poa')
    @consumer_key = consumer_key
    @access_token = access_token
    @access_token_secret = access_token_secret
    @signature_key = rsa(signature_key)
    @encryption_key = rsa(encryption_key)
    @dh_prime = dh_prime
    @dh_generator = dh_generator
    @realm = realm
  end

  attr_reader :consumer_key, :access_token, :realm

  # --- Pure helpers (class methods) ---

  # Integer -> byte array, matching IBKR's reference exactly: pad to even hex length, and
  # prepend a leading 0x00 when the bit length is a multiple of 8 (keeps the value positive).
  def self.to_byte_array(int)
    hex = int.to_s(16)
    hex = "0#{hex}" if hex.length.odd?
    bytes = []
    bytes << 0 if (int.to_s(2).length % 8).zero?
    hex.scan(/../).each { |pair| bytes << pair.to_i(16) }
    bytes
  end

  # Percent-encode like Python urllib.parse.quote_plus: encode every byte outside the
  # unreserved set as upper-case %XX, and turn spaces into "+".
  def self.escape(str)
    str.to_s.b.gsub(UNRESERVED) { |c| c.bytes.map { |b| format('%%%02X', b) }.join }
       .tr(' ', '+')
       .force_encoding(Encoding::UTF_8)
  end

  # OAuth 1.0a signature base string: METHOD & quote_plus(url) & quote_plus(sorted "k=v" joined by "&").
  # `prepend` is concatenated verbatim at the front (used by the live-session-token request).
  def self.base_string(method:, url:, params:, prepend: nil)
    sorted = params.sort_by { |k, _| k.to_s }.map { |k, v| "#{k}=#{v}" }.join('&')
    base = [method, escape(url), escape(sorted)].join('&')
    prepend ? "#{prepend}#{base}" : base
  end

  # HMAC-SHA256 signature for protected resources, keyed by the base64-decoded live session token.
  def self.hmac_sha256_signature(base_string, live_session_token)
    key = Base64.decode64(live_session_token)
    escape(Base64.strict_encode64(OpenSSL::HMAC.digest('SHA256', key, base_string)))
  end

  # Derive the live session token from the Diffie-Hellman exchange:
  # shared = dh_response^dh_random mod dh_prime, then base64( HMAC-SHA1(key = bytes(shared), msg = prepend bytes) ).
  def self.calculate_live_session_token(dh_prime:, dh_random:, dh_response:, prepend:)
    shared = dh_response.to_i(16).pow(dh_random.to_i(16), dh_prime.to_i(16))
    key = to_byte_array(shared).pack('C*')
    msg = [prepend].pack('H*')
    Base64.strict_encode64(OpenSSL::HMAC.digest('SHA1', key, msg))
  end

  # Verify the LST the server returns: HMAC-SHA1(key = base64-decoded LST, msg = consumer_key) hexdigest.
  def self.validate_live_session_token(live_session_token, signature:, consumer_key:)
    computed = OpenSSL::HMAC.hexdigest('SHA1', Base64.decode64(live_session_token), consumer_key)
    ActiveSupport::SecurityUtils.secure_compare(computed, signature)
  rescue ArgumentError
    false
  end

  def self.nonce
    SecureRandom.alphanumeric(16)
  end

  def self.timestamp
    Time.now.to_i.to_s
  end

  # --- Instance methods (need the credentials/keys) ---

  # RSA-SHA256 signature (used to obtain the live session token): PKCS#1 v1.5 over SHA-256,
  # base64 (no newlines), then url-encoded.
  def rsa_sha256_signature(base_string)
    signature = @signature_key.sign(OpenSSL::Digest.new('SHA256'), base_string)
    self.class.escape(Base64.strict_encode64(signature))
  end

  # Assemble the Authorization header: `OAuth realm="...", k1="v1", k2="v2", ...` (params sorted by key).
  def authorization_header(params)
    pairs = params.sort_by { |k, _| k.to_s }.map { |k, v| %(#{k}="#{v}") }.join(', ')
    %(OAuth realm="#{@realm}", #{pairs})
  end

  private

  def rsa(key)
    key.is_a?(OpenSSL::PKey::RSA) ? key : OpenSSL::PKey::RSA.new(key)
  end
end
