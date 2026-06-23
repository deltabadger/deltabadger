# Shared template for the limit-check jobs: while a bot is :waiting, poll the
# type-specific limit condition and either hand off to Bot::ActionJob or reschedule.
# Subclasses keep their own class names (Solid Queue serializes them into queued
# jobs) and override condition_result and next_check_at.
class Bot::LimitCheckJobBase < ApplicationJob
  queue_as :default

  def perform(bot)
    return unless bot.waiting?

    result = condition_result(bot)
    if result.failure?
      reschedule_after_transient(bot, result.errors.to_sentence)
      return
    end

    if result.data
      bot.update!(status: :scheduled)
      Bot::ActionJob.perform_later(bot)
    else
      self.class.set(wait_until: next_check_at(bot)).perform_later(bot)
    end
  rescue Client::TransientNetworkError, Client::RateLimitedError => e
    # A live-price read raised a transient (network blip / rate limit) instead of returning a
    # failure Result. Without this rescue the raise escapes perform, the job dead-letters, and
    # the self-rescheduling poll chain — the ONLY thing re-polling a :waiting limit bot — stops,
    # wedging the bot in :waiting forever. Treat it exactly like result.failure?: reschedule the
    # poll in 1 minute, bot stays :waiting. We deliberately rescue ONLY these typed errors so a
    # genuine bug still dead-letters and surfaces.
    reschedule_after_transient(bot, e.message)
  end

  private

  def reschedule_after_transient(bot, reason)
    Rails.logger.warn("#{self.class.name.demodulize} for bot #{bot.id} failed: #{reason}. Retrying in 1 minute.")
    self.class.set(wait_until: 1.minute.from_now).perform_later(bot)
  end

  def condition_result(bot)
    raise NotImplementedError
  end

  def next_check_at(bot)
    raise NotImplementedError
  end
end
