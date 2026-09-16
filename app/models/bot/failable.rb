## Two pieces of state with deliberately DIFFERENT lifetimes.
#
# `last_failure_kind` is the strike. Every failing exit of Bot::ActionJob writes it, and a
# successful run clears it — so "two in a row" means two in a row, not "two ever". Without the
# clear on success, a bot that failed an auth call in March would be one strike from a permanent
# stop forever.
#
# `failure_notifications` is the mail budget, one timestamp per kind, and is NEVER cleared on
# success — only aged out. Clearing it was the obvious shortcut and it is wrong: fail → succeed →
# fail the same way an hour later would mail twice, and so would funds → unknown → funds under a
# single scalar, because the kind "changed" each time.
#
# Both live in `transient_data` rather than new columns: hosted containers do not run migrations
# under the default `web` command, and this is exactly the per-bot job state that column is for
# (Bot::Rebalanceable stores its timestamps the same way). Always through merge_transient_data!,
# which locks and merges against committed state — a plain read-modify-write here can erase the
# placement-intent key, which is the only thing standing between an accepted-but-unrecorded order
# and a second one on top of it.
module Bot::Failable
  # Kinds the user must act on: the venue is refusing the credentials, the scope, or the asset.
  # Retrying cannot fix any of them, so a bot that keeps hitting one is stopped rather than left
  # failing on a schedule.
  BLOCKING_KINDS = %i[invalid_key permission_denied restricted].freeze

  KIND_KEY = 'last_failure_kind'.freeze
  NOTIFICATIONS_KEY = 'failure_notifications'.freeze
  UNKNOWN_KIND = 'unknown'.freeze

  def last_failure_kind
    transient_data[KIND_KEY]
  end

  def failure_notifications
    transient_data[NOTIFICATIONS_KEY] || {}
  end

  # A blocking kind stops the bot only on its SECOND consecutive occurrence. One auth rejection is
  # genuinely ambiguous on most venues — Binance and Bybit file a wrong IP, a missing scope and a
  # revoked key under one string; Alpaca's whole invalid-key bucket is the substring "unauthorized";
  # IBKR 401s identically for a competing login, an expired session and a key still activating.
  # Stopping a fleet on a one-off blip is far worse than trading one extra interval.
  def blocking_failure?(kind)
    kind.present? && BLOCKING_KINDS.include?(kind.to_sym) && last_failure_kind == kind.to_s
  end

  def notify_about_failure?(kind)
    # Buy-side out-of-funds is the one email Bot::Fundable already budgets, through
    # last_end_of_funds_notification. Two independent day-long throttles on one email is two emails
    # a day, so this path shares that budget instead of opening a second one.
    #
    # Sell-side does NOT share it: a selling bot spends BASE and its rejection goes out as
    # notify_about_error, while Fundable's window is scoped by QUOTE asset across all the user's
    # bots — so sharing would let a buying bot's low-USD warning swallow a selling bot's first
    # insufficient-BTC alert, and writing to it from here would swallow the next USD one.
    return !notified_in_last_day? if shares_end_of_funds_budget?(kind)

    notified_at = failure_notifications[failure_kind_key(kind)]
    notified_at.blank? || Time.zone.parse(notified_at) < 1.day.ago
  end

  # `notified:` is the decision the caller already made and acted on, passed in rather than
  # recomputed — notify_about_failure? must be read BEFORE this writes the row that would answer it.
  def record_failure!(kind, notified: false)
    values = { KIND_KEY => kind.presence&.to_s }

    if notified && shares_end_of_funds_budget?(kind)
      # update_column, not update!: Bot::Accountable raises from a before_save whenever settings are
      # dirty, and a freshly loaded bot usually is.
      update_column(:last_end_of_funds_notification, Time.current)
    elsif notified
      values[NOTIFICATIONS_KEY] = failure_notifications.merge(failure_kind_key(kind) => Time.current.iso8601)
    end

    merge_transient_data!(values)
  end

  # nil deletes the key (merge_transient_data! compacts), so this leaves no empty entry behind. The
  # guard keeps a successful tick from taking the row lock on every single run.
  def clear_failure_state!
    return if last_failure_kind.blank?

    merge_transient_data!(KIND_KEY => nil)
  end

  private

  def failure_kind_key(kind)
    kind.presence&.to_s || UNKNOWN_KIND
  end

  def shares_end_of_funds_budget?(kind)
    kind.to_s == 'insufficient_funds' && !try(:selling?) && respond_to?(:notified_in_last_day?)
  end
end
