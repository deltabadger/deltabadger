require 'test_helper'

class Clients::Ibkr::SessionTest < ActiveSupport::TestCase
  BASE = 'https://api.ibkr.com'.freeze
  CONSUMER = 'TESTCONS1'.freeze

  # Generated once (2048-bit RSA gen is slow to repeat); fixed 1024-bit DH params embedded.
  SIG_KEY = OpenSSL::PKey::RSA.new(2048)
  ENC_KEY = OpenSSL::PKey::RSA.new(2048)
  DH_PARAM = <<~PEM.freeze
    -----BEGIN DH PARAMETERS-----
    MIGLAoGBAPSHspMWc9/phm3D2OBRkX50s+m2FIhbYg3DuXLrMg3lmEZlxRNVh1e2
    c3uCWZcYlMvVr1WlIjavZDukxnQJ03+l4UbiwnShAyEOtDcx+CF2AX9EW8+56seh
    kiWLnuA42ENa2+S67sxqItRI1s4IuFD6zK4+zGycXR8EABoRrZh3AgECAgIArw==
    -----END DH PARAMETERS-----
  PEM

  PREPEND_PLAIN = ['00a1b2c3d4e5f6'].pack('H*').freeze # leading 0x00 to exercise byte handling
  PREPEND_HEX = PREPEND_PLAIN.unpack1('H*').freeze

  ApiKeyStub = Struct.new(:id, :key, :access_token, :secret, :rsa_signature_key,
                          :rsa_encryption_key, :dh_param, :ibkr_realm, keyword_init: true)

  setup do
    @prev_cache = Rails.cache
    Rails.cache = ActiveSupport::Cache::MemoryStore.new # real cache so caching is observable

    access_token_secret = Base64.strict_encode64(
      ENC_KEY.public_key.public_encrypt(PREPEND_PLAIN, OpenSSL::PKey::RSA::PKCS1_PADDING)
    )
    @api_key = ApiKeyStub.new(
      id: 1, key: CONSUMER, access_token: 'access-token', secret: access_token_secret,
      rsa_signature_key: SIG_KEY.to_pem, rsa_encryption_key: ENC_KEY.to_pem,
      dh_param: DH_PARAM, ibkr_realm: 'limited_poa'
    )
    @session = Clients::Ibkr::Session.new(api_key: @api_key)
  end

  teardown do
    Rails.cache = @prev_cache
    WebMock.reset!
  end

  # The IBKR server side of the DH exchange: read the client's challenge from the Authorization
  # header, pick a server secret b, return g^b and the LST signature so the client can verify.
  def stub_live_session_token(signature_override: nil)
    dh = OpenSSL::PKey::DH.new(DH_PARAM)
    prime = dh.p.to_i
    g = dh.g.to_i
    b = 0x1f2e3d4c5b6a7988

    stub_request(:post, "#{BASE}#{Clients::Ibkr::Session::LST_PATH}").to_return do |request|
      challenge = request.headers['Authorization'][/diffie_hellman_challenge="([0-9a-fA-F]+)"/, 1].to_i(16)
      dh_response = g.pow(b, prime)
      shared = challenge.pow(b, prime)
      lst = Base64.strict_encode64(
        OpenSSL::HMAC.digest('SHA1', Clients::Ibkr::Oauth.to_byte_array(shared).pack('C*'), [PREPEND_HEX].pack('H*'))
      )
      signature = signature_override || OpenSSL::HMAC.hexdigest('SHA1', Base64.decode64(lst), CONSUMER)
      {
        status: 200, headers: { 'Content-Type' => 'application/json' },
        body: { diffie_hellman_response: dh_response.to_s(16),
                live_session_token_signature: signature,
                live_session_token_expiration: 86_400_000 }.to_json
      }
    end
  end

  def stub_ssodh_init
    stub_request(:post, "#{BASE}#{Clients::Ibkr::Session::SSODH_INIT_PATH}")
      .to_return(status: 200, headers: { 'Content-Type' => 'application/json' },
                 body: { authenticated: true, connected: true, competing: false }.to_json)
  end

  test 'mints and validates the live session token end-to-end' do
    stub_live_session_token
    lst = @session.live_session_token
    assert_match(%r{\A[A-Za-z0-9+/=]+\z}, lst)
    assert_requested :post, "#{BASE}#{Clients::Ibkr::Session::LST_PATH}", times: 1
  end

  test 'caches the LST — a second call does not hit the network' do
    stub_live_session_token
    first = @session.live_session_token
    second = @session.live_session_token
    assert_equal first, second
    assert_requested :post, "#{BASE}#{Clients::Ibkr::Session::LST_PATH}", times: 1
  end

  test 'raises AuthError when the live session token signature does not validate' do
    stub_live_session_token(signature_override: 'deadbeef')
    assert_raises(Clients::Ibkr::Session::AuthError) { @session.live_session_token }
  end

  test 'signed_request opens the brokerage session then calls the endpoint' do
    stub_live_session_token
    stub_ssodh_init
    accounts = stub_request(:get, "#{BASE}/v1/api/iserver/accounts")
               .to_return(status: 200, headers: { 'Content-Type' => 'application/json' },
                          body: { accounts: ['U123'] }.to_json)

    body = @session.signed_request(:get, '/v1/api/iserver/accounts')

    assert_equal ['U123'], body['accounts']
    assert_requested :post, "#{BASE}#{Clients::Ibkr::Session::SSODH_INIT_PATH}", times: 1
    assert_requested accounts, times: 1
  end

  test 'self-heals once on a 401: re-mints LST + re-inits session, then succeeds' do
    stub_live_session_token
    stub_ssodh_init
    stub_request(:get, "#{BASE}/v1/api/iserver/accounts").to_return(
      { status: 401, body: '{"error":"not authenticated"}', headers: { 'Content-Type' => 'application/json' } },
      { status: 200, body: { accounts: ['U123'] }.to_json, headers: { 'Content-Type' => 'application/json' } }
    )

    body = @session.signed_request(:get, '/v1/api/iserver/accounts')

    assert_equal ['U123'], body['accounts']
    assert_requested :get, "#{BASE}/v1/api/iserver/accounts", times: 2
    # invalidation forced a re-mint of the LST and a re-init of the brokerage session
    assert_requested :post, "#{BASE}#{Clients::Ibkr::Session::LST_PATH}", times: 2
    assert_requested :post, "#{BASE}#{Clients::Ibkr::Session::SSODH_INIT_PATH}", times: 2
  end
end
