# frozen_string_literal: true

# Stores idempotent request claims for trade-style POSTs that must not be
# re-executed on retry. The concern (`Idempotency`) is the only intended
# consumer — see comments there for the locking/replay protocol.
class IdempotencyKey < ApplicationRecord
  belongs_to :user

  enum :state, { in_progress: 'in_progress', completed: 'completed' }

  validates :key, presence: true
  validates :request_fingerprint, presence: true
  validates :locked_at, presence: true
  validates :key, uniqueness: { scope: :user_id }
end
