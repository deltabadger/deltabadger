# §10 connect wizard, step 1. Generates the per-user first-party-OAuth artifacts IBKR needs:
# two RSA-2048 keypairs (signing + encryption) and Diffie-Hellman params. We keep the private
# keys (encrypted on the ApiKey); their public halves + the DH params are what the user uploads
# to IBKR. DH safe-prime generation is the slow step, which is why this is a background job.
#
# generate_rsa / generate_dh_params are class methods so tests can stub the slow DH step.
class Ibkr::GenerateConnectionKeysJob < ApplicationJob
  queue_as :default

  RSA_BITS = 2048
  DH_BITS = 2048

  def perform(api_key_id)
    api_key = ApiKey.find_by(id: api_key_id)
    return unless api_key
    # Idempotent: a prior job (or a double-clicked start) may already have generated the keys.
    # Never rotate artifacts the user may have downloaded + uploaded to IBKR — that would leave
    # IBKR holding public keys that no longer match our stored private keys.
    return if api_key.rsa_signature_key.present?

    api_key.update!(
      rsa_signature_key: self.class.generate_rsa.to_pem,
      rsa_encryption_key: self.class.generate_rsa.to_pem,
      dh_param: self.class.generate_dh_params.to_pem
    )
    broadcast_ready(api_key)
  end

  def self.generate_rsa
    OpenSSL::PKey::RSA.new(RSA_BITS)
  end

  def self.generate_dh_params
    OpenSSL::PKey::DH.new(DH_BITS)
  end

  private

  # Reveal the download panel on the open wizard once the artifacts are ready.
  def broadcast_ready(api_key)
    Turbo::StreamsChannel.broadcast_replace_to(
      "user_#{api_key.user_id}", :ibkr_connection,
      target: 'ibkr-connect-wizard',
      partial: 'settings/ibkr_connections/wizard',
      locals: { api_key: api_key }
    )
  rescue StandardError => e
    Rails.logger.warn("IBKR key-gen broadcast failed for api_key #{api_key.id}: #{e.message}")
  end
end
