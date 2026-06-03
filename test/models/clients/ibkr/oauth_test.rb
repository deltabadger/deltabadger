require 'test_helper'

# Test contract for the IBKR Web API OAuth 1.0a signing module.
#
# IBKR's first-party / self-service OAuth 1.0a is the gateway-free path that lets a
# user trade their OWN account over https://api.ibkr.com (no Java IB Gateway, no browser).
# The signing is the load-bearing, highest-risk piece, so these tests pin the byte-exact
# behaviour BEFORE the implementation exists (TDD red state — Clients::Ibkr::Oauth is not
# written yet; this file errors until it is).
#
# All expected vectors below were computed INDEPENDENTLY with Python's stdlib
# (hashlib/hmac/urllib) against the reference implementation IBKR ships
# (ibind oauth1a.py — "based on code provided directly by IBKR"), so they are a genuine
# cross-language cross-check, not a restatement of the Ruby implementation.
#
# Signing recipe being pinned:
#   - base string      = METHOD & quote_plus(url) & quote_plus(sorted "k=v" joined by "&")
#   - request signature= HMAC-SHA256(key = base64-decoded live session token), url-encoded base64
#   - live session token: DH shared secret = dh_response^dh_random mod dh_prime,
#                         then base64( HMAC-SHA1(key = bytes(to_byte_array(shared)), msg = prepend bytes) )
#   - LST validation   = HMAC-SHA1(key = base64-decoded LST, msg = consumer_key).hexdigest
#   - escaping         = Python urllib quote_plus semantics (space->"+", "~" stays, "/"->"%2F")
class Clients::Ibkr::OauthTest < ActiveSupport::TestCase
  # Generated once at load time (2048-bit RSA key generation is slow to repeat per test).
  RSA_KEY = OpenSSL::PKey::RSA.new(2048)

  setup do
    @oauth = Clients::Ibkr::Oauth.new(
      consumer_key: 'TESTCONS',
      access_token: 'token',
      access_token_secret: 'c2VjcmV0', # base64; only used by the instance LST path, not these unit tests
      signature_key: RSA_KEY.to_pem,
      encryption_key: RSA_KEY.to_pem,
      dh_prime: '00cc8f1bbe20a6993bb1e2d89f0f1b2b6f8b9b5d3a7c1e4f6079123456789abcd',
      realm: 'limited_poa' # NOTE: confirm the exact first-party realm against a live IBKR account in Phase 0
    )
  end

  # --- to_byte_array: the flagged porting risk (zero-pad when bit length is a multiple of 8) ---
  test 'to_byte_array matches IBKR reference padding semantics' do
    {
      0 => [0], 1 => [1], 16 => [16], 127 => [127],
      128 => [0, 128], 255 => [0, 255], 256 => [1, 0],
      65_535 => [0, 255, 255], 65_536 => [1, 0, 0]
    }.each do |int, bytes|
      assert_equal bytes, Clients::Ibkr::Oauth.to_byte_array(int), "to_byte_array(#{int})"
    end
  end

  # --- escape: must match Python quote_plus exactly, or every signature breaks ---
  test 'escape matches python quote_plus (space, tilde, slash, reserved)' do
    assert_equal 'a+b~c%2Fd', Clients::Ibkr::Oauth.escape('a b~c/d')
    assert_equal 'AAPL%3D100%26x_y.z~', Clients::Ibkr::Oauth.escape('AAPL=100&x_y.z~')
    assert_equal 'https%3A%2F%2Fapi.ibkr.com%2Fv1%2Fapi',
                 Clients::Ibkr::Oauth.escape('https://api.ibkr.com/v1/api')
  end

  # --- base string: sorted params, "k=v" joined by "&", whole thing quote_plus'd ---
  test 'base_string builds METHOD&escaped_url&escaped_sorted_params' do
    bs = Clients::Ibkr::Oauth.base_string(
      method: 'GET',
      url: 'https://api.ibkr.com/v1/api/portfolio/accounts',
      params: { 'oauth_nonce' => 'abc', 'oauth_consumer_key' => 'TESTCONS' } # intentionally unsorted
    )
    assert_equal(
      'GET&https%3A%2F%2Fapi.ibkr.com%2Fv1%2Fapi%2Fportfolio%2Faccounts&' \
      'oauth_consumer_key%3DTESTCONS%26oauth_nonce%3Dabc',
      bs
    )
  end

  test 'base_string prepends the prepend (used for the live-session-token request)' do
    bs = Clients::Ibkr::Oauth.base_string(
      method: 'POST', url: 'https://api.ibkr.com/x', params: { 'a' => '1' }, prepend: 'PREPEND'
    )
    assert bs.start_with?('PREPEND')
  end

  # --- HMAC-SHA256 request signature (protected resources), known-answer ---
  test 'hmac_sha256_signature produces the url-encoded base64 signature' do
    base_string = Clients::Ibkr::Oauth.base_string(
      method: 'GET',
      url: 'https://api.ibkr.com/v1/api/portfolio/accounts',
      params: { 'oauth_consumer_key' => 'TESTCONS', 'oauth_nonce' => 'abc' }
    )
    sig = Clients::Ibkr::Oauth.hmac_sha256_signature(base_string, 'bGl2ZXNlc3Npb250b2tlbg==')
    assert_equal 'FuskAI5IUwEzj3ciHIRcKn%2FGmm%2B9daeFu2ip2V96bUU%3D', sig
  end

  # --- RSA-SHA256 signature (used to obtain the live session token), roundtrip-verifiable ---
  test 'rsa_sha256_signature is verifiable with the public key and has no newlines' do
    base_string = 'POST&https%3A%2F%2Fexample.com&oauth_consumer_key%3DTESTCONS'
    sig = @oauth.rsa_sha256_signature(base_string)

    refute_includes sig, "\n", 'signature must not contain newlines'
    decoded = Base64.decode64(CGI.unescape(sig))
    assert RSA_KEY.public_key.verify(OpenSSL::Digest.new('SHA256'), decoded, base_string),
           'RSA-SHA256 signature must verify against the public signature key'
  end

  # --- Diffie-Hellman live session token, known-answer (Python-verified, both DH sides agree) ---
  test 'calculate_live_session_token derives the LST from the DH exchange' do
    token = Clients::Ibkr::Oauth.calculate_live_session_token(
      dh_prime: '00cc8f1bbe20a6993bb1e2d89f0f1b2b6f8b9b5d3a7c1e4f6079123456789abcd',
      dh_random: '3a7f9c2d1e',
      dh_response: '6dcbd0d9d2a0a2860b9df30955d152a0776c9ac29bfa62c5900e46abd98717c',
      prepend: 'deadbeef0011'
    )
    assert_equal '9stTxQv6fGt6TDQLWyVnlKn2M7o=', token
  end

  # --- live session token validation (HMAC-SHA1 of consumer key, keyed by the decoded LST) ---
  test 'validate_live_session_token confirms a matching signature and rejects a bad one' do
    lst = 'bGl2ZXNlc3Npb250b2tlbg=='
    assert Clients::Ibkr::Oauth.validate_live_session_token(
      lst, signature: '465b8c3dd799d2f14ac350df1bcdf6c552289341', consumer_key: 'TESTCONS'
    )
    refute Clients::Ibkr::Oauth.validate_live_session_token(
      lst, signature: 'deadbeef', consumer_key: 'TESTCONS'
    )
  end

  # --- nonce / timestamp formats ---
  test 'nonce is 16 alphanumeric chars and varies' do
    assert_match(/\A[A-Za-z0-9]{16}\z/, Clients::Ibkr::Oauth.nonce)
    refute_equal Clients::Ibkr::Oauth.nonce, Clients::Ibkr::Oauth.nonce
  end

  test 'timestamp is unix seconds as a string' do
    assert_match(/\A\d{10}\z/, Clients::Ibkr::Oauth.timestamp)
  end

  # --- prepend: decrypt access-token secret with the encryption key (the path the rest skips) ---
  test 'prepend decrypts the access-token secret to hex, preserving leading zero bytes' do
    plaintext = ['00ff10'].pack('H*') # leading 0x00 byte must survive
    enc_secret = Base64.strict_encode64(
      RSA_KEY.public_key.public_encrypt(plaintext, OpenSSL::PKey::RSA::PKCS1_PADDING)
    )
    oauth = Clients::Ibkr::Oauth.new(
      consumer_key: 'C', access_token: 't', access_token_secret: enc_secret,
      signature_key: RSA_KEY.to_pem, encryption_key: RSA_KEY.to_pem,
      dh_prime: '00cc8f1bbe20a6993bb1e2d89f0f1b2b6f8b9b5d3a7c1e4f6079123456789abcd'
    )
    assert_equal '00ff10', oauth.prepend
  end

  # --- dh_challenge: g^random mod prime, against the Python-computed vector ---
  test 'dh_challenge matches the known DH vector' do
    assert_equal '2b66064b7cfdf414ab0d8ff44103e42b46637c7402d69bd04b2dc2d7da8a9aa',
                 @oauth.dh_challenge('3a7f9c2d1e')
  end

  test 'dh_random is 64 hex chars' do
    assert_match(/\A[0-9a-f]{64}\z/, Clients::Ibkr::Oauth.dh_random)
  end

  # --- request header builders ---
  test 'live_session_token_header is an RSA-SHA256 OAuth header carrying the DH challenge' do
    header = @oauth.live_session_token_header(
      url: 'https://api.ibkr.com/v1/api/oauth/live_session_token',
      dh_challenge: 'abc123', prepend: 'deadbeef'
    )
    assert header.start_with?('OAuth realm="limited_poa", ')
    assert_includes header, 'diffie_hellman_challenge="abc123"'
    assert_includes header, 'oauth_signature_method="RSA-SHA256"'
    assert_includes header, 'oauth_consumer_key="TESTCONS"'
    assert_match(/oauth_signature="[^"]+"/, header)
  end

  test 'signed_header is an HMAC-SHA256 OAuth header and does not leak query params' do
    header = @oauth.signed_header(
      method: 'GET', url: 'https://api.ibkr.com/v1/api/iserver/accounts',
      live_session_token: 'bGl2ZXNlc3Npb250b2tlbg==', query_params: { 'fields' => 'secret_value' }
    )
    assert_includes header, 'oauth_signature_method="HMAC-SHA256"'
    assert_match(/oauth_signature="[^"]+"/, header)
    refute_includes header, 'secret_value' # query params sign the base string but aren't in the header
  end

  # --- Authorization header assembly: OAuth realm + sorted key="value" pairs ---
  test 'authorization_header assembles the OAuth realm header with sorted params' do
    header = @oauth.authorization_header(
      'oauth_token' => 'tok',
      'oauth_consumer_key' => 'TESTCONS',
      'oauth_signature_method' => 'HMAC-SHA256',
      'oauth_nonce' => 'abc',
      'oauth_timestamp' => '1700000000',
      'oauth_signature' => 'SIG'
    )
    assert_equal(
      'OAuth realm="limited_poa", oauth_consumer_key="TESTCONS", oauth_nonce="abc", ' \
      'oauth_signature="SIG", oauth_signature_method="HMAC-SHA256", ' \
      'oauth_timestamp="1700000000", oauth_token="tok"',
      header
    )
  end
end
