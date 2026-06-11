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
      Rails.logger.warn("#{self.class.name.demodulize} for bot #{bot.id} failed: #{result.errors.to_sentence}. Retrying in 1 minute.")
      self.class.set(wait_until: 1.minute.from_now).perform_later(bot)
      return
    end

    if result.data
      bot.update!(status: :scheduled)
      Bot::ActionJob.perform_later(bot)
    else
      self.class.set(wait_until: next_check_at(bot)).perform_later(bot)
    end
  end

  private

  def condition_result(bot)
    raise NotImplementedError
  end

  def next_check_at(bot)
    raise NotImplementedError
  end
end
