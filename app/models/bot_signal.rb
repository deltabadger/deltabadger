class BotSignal < ApplicationRecord
  # One call per rule per this window. It sits under TradingView's finest sane alert cadence (once
  # per minute) and above the seconds a retrying sender waits — the replay guard for a receiver
  # whose one universal caller cannot be asked for an idempotency key.
  # ponytail: one fixed window; make it a per-rule setting if a trader asks for a faster cadence.
  TRIGGER_COOLDOWN = 30.seconds

  belongs_to :bot

  enum :direction, { buy: 0, sell: 1 }
  enum :amount_type, { fixed: 0, percentage: 1 }

  validates :token, presence: true, uniqueness: true
  validates :direction, presence: true
  validates :amount, presence: true, numericality: { greater_than: 0 }
  validates :amount, numericality: { less_than_or_equal_to: 100 }, if: :percentage?

  before_validation :generate_token, on: :create

  # Served by HooksController (POST only, outside the locale scope). See docs/signal-bots.md.
  def webhook_url
    "/hook/#{token}"
  end

  # The claim a call has to win before anything is enqueued: one conditional UPDATE, so of two
  # concurrent calls exactly one sees a row change. Nothing is read first, nothing is locked.
  def claim_trigger!(now: Time.current)
    claimed = BotSignal.where(id:)
                       .where('last_triggered_at IS NULL OR last_triggered_at <= ?', now - TRIGGER_COOLDOWN)
                       .update_all(last_triggered_at: now) == 1
    self.last_triggered_at = now if claimed
    claimed
  end

  # Hand a claim back when the call could not be kept — but only while it is still ours. A request
  # that stalled long enough for the cooldown to lapse and another call to claim must not release
  # that newer claim along with its own.
  def release_trigger!(previous:)
    released = BotSignal.where(id:, last_triggered_at:).update_all(last_triggered_at: previous) == 1
    self.last_triggered_at = previous if released
    released
  end

  # Why a call that won the claim would still do nothing, or nil when it would run.
  def ignore_reason
    return 'bot_not_running' unless bot.working?
    return 'signal_disabled' unless enabled?

    nil
  end

  private

  def generate_token
    return if token.present?

    loop do
      self.token = SecureRandom.urlsafe_base64(32)
      break unless BotSignal.exists?(token: token)
    end
  end
end
