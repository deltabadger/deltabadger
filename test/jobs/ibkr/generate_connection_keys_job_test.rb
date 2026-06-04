require 'test_helper'

# §10 connect wizard, step 1. The slow part of connecting IBKR is generating the per-user
# RSA keypairs + Diffie-Hellman params server-side, so it runs as a background job. This proves
# the job produces artifacts that (a) are valid keys and (b) the existing Clients::Ibkr::Session
# can actually consume.
class Ibkr::GenerateConnectionKeysJobTest < ActiveSupport::TestCase
  # Reuse the Session test's known-good 1024-bit DH params (generator 2) so the job's slow
  # 2048-bit safe-prime generation is stubbed out and the test stays fast.
  DH_PARAM = <<~PEM.freeze
    -----BEGIN DH PARAMETERS-----
    MIGLAoGBAPSHspMWc9/phm3D2OBRkX50s+m2FIhbYg3DuXLrMg3lmEZlxRNVh1e2
    c3uCWZcYlMvVr1WlIjavZDukxnQJ03+l4UbiwnShAyEOtDcx+CF2AX9EW8+56seh
    kiWLnuA42ENa2+S67sxqItRI1s4IuFD6zK4+zGycXR8EABoRrZh3AgECAgIArw==
    -----END DH PARAMETERS-----
  PEM

  setup do
    @user = create(:user)
    @ibkr = create(:ibkr_exchange)
    @api_key = @user.api_keys.create!(exchange: @ibkr, key_type: :trading, status: :pending_validation)
    Ibkr::GenerateConnectionKeysJob.stubs(:generate_dh_params).returns(OpenSSL::PKey::DH.new(DH_PARAM))
  end

  test 'fills the api key with valid RSA signing + encryption private keys and DH params' do
    Ibkr::GenerateConnectionKeysJob.perform_now(@api_key.id)
    @api_key.reload

    assert OpenSSL::PKey::RSA.new(@api_key.rsa_signature_key).private?, 'signing key is a private RSA key'
    assert OpenSSL::PKey::RSA.new(@api_key.rsa_encryption_key).private?, 'encryption key is a private RSA key'
    assert_not_equal @api_key.rsa_signature_key, @api_key.rsa_encryption_key, 'distinct keypairs'

    assert_equal 2, OpenSSL::PKey::DH.new(@api_key.dh_param).g.to_i, 'DH generator is 2 (Session expects this)'
  end

  test 'the generated artifacts are consumable by Clients::Ibkr::Session without raising' do
    Ibkr::GenerateConnectionKeysJob.perform_now(@api_key.id)
    @api_key.reload

    assert_nothing_raised { Clients::Ibkr::Session.new(api_key: @api_key) }
  end

  test 'is idempotent — a duplicate job does not rotate already-generated artifacts' do
    Ibkr::GenerateConnectionKeysJob.perform_now(@api_key.id)
    sig = @api_key.reload.rsa_signature_key

    # A second job (e.g. from a double-clicked "start") must leave the existing keys untouched,
    # so public keys the user already uploaded to IBKR keep matching our private keys.
    Ibkr::GenerateConnectionKeysJob.perform_now(@api_key.id)
    assert_equal sig, @api_key.reload.rsa_signature_key
  end
end
