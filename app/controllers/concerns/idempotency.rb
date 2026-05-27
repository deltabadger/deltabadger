# frozen_string_literal: true

# Stripe-style idempotency for write endpoints (notably POST /api/v1/orders).
# Required protocol:
#
#   - Client MUST send `Idempotency-Key: <opaque>`. Missing → 400.
#   - First call: claim a row in :in_progress, run the block outside any
#     transaction, then update to :completed with the serialized response.
#     Re-render the same bytes back to the caller.
#   - Same key in :in_progress: 409 idempotency_in_progress. We do not block
#     and wait — keep semantics simple, let the client retry.
#   - Same key in :completed, same fingerprint: replay byte-for-byte using
#     `render body:` (never `render json:`, which would re-encode the stored
#     JSON string and double-quote it).
#   - Same key in :completed, different fingerprint: 409 idempotency_key_reused.
#   - Failed exchange responses ARE stored and replayed — an exchange-rejected
#     order is a definitive outcome; replaying the failure is safer than
#     re-submitting blindly.
#
# Controllers wrap their action body with `idempotent_request` and return
# `[http_status_int, body_hash]` from the block.
module Idempotency
  extend ActiveSupport::Concern

  HEADER = 'Idempotency-Key'

  def self.fingerprint(params)
    normalized = deep_sort_for_fingerprint(params)
    Digest::SHA256.hexdigest(JSON.generate(normalized))
  end

  def self.deep_sort_for_fingerprint(value)
    case value
    when Hash then value.transform_keys(&:to_s).sort.to_h.transform_values { |v| deep_sort_for_fingerprint(v) }
    when Array then value.map { |v| deep_sort_for_fingerprint(v) }
    else value
    end
  end

  private

  def idempotent_request
    key = request.headers[HEADER].to_s
    return render_idem_error(:bad_request, 'idempotency_key_required', "#{HEADER} header is required.") if key.blank?

    fingerprint = Idempotency.fingerprint(idempotency_fingerprint_payload)
    record = claim_idempotency_key(key, fingerprint)
    return unless record # error already rendered

    status, body = yield
    body_json = JSON.generate(body)

    IdempotencyKey.where(id: record.id).update_all(
      state: 'completed',
      response_status: status,
      response_body: body_json,
      updated_at: Time.current
    )

    render body: body_json, status: status, content_type: 'application/json'
  end

  # Params used for fingerprinting. Subclasses may override to scope it
  # (e.g. exclude pagination). Default: the body params.
  def idempotency_fingerprint_payload
    request.request_parameters
  end

  def claim_idempotency_key(key, fingerprint)
    IdempotencyKey.create!(
      user_id: current_user.id, key: key,
      request_fingerprint: fingerprint, state: 'in_progress',
      locked_at: Time.current
    )
  rescue ActiveRecord::RecordInvalid, ActiveRecord::RecordNotUnique
    handle_existing_idempotency_key(key, fingerprint)
  end

  def handle_existing_idempotency_key(key, fingerprint)
    existing = IdempotencyKey.find_by(user_id: current_user.id, key: key)
    # Race: row was destroyed between our insert collision and this lookup.
    # Treat as in-progress conflict — safer than re-running.
    return render_idem_conflict(:idempotency_in_progress) if existing.nil?

    if existing.in_progress?
      render_idem_conflict(:idempotency_in_progress)
    elsif existing.request_fingerprint != fingerprint
      render_idem_conflict(:idempotency_key_reused)
    else
      replay_idempotency(existing)
    end
    nil
  end

  def replay_idempotency(record)
    render body: record.response_body,
           status: record.response_status,
           content_type: 'application/json'
  end

  def render_idem_conflict(code)
    message = if code == :idempotency_in_progress
                'A request with this Idempotency-Key is already in progress.'
              else
                'This Idempotency-Key was used with a different request payload.'
              end
    render_idem_error(:conflict, code.to_s, message)
  end

  def render_idem_error(status, code, message)
    render json: { data: nil, error: { code: code, message: message } }, status: status
  end
end
