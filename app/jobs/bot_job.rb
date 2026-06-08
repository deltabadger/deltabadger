class BotJob < ApplicationJob
  limits_concurrency to: 1, key: ->(bot, *) { "exchange_#{bot.exchange&.name_id}" }

  # Escalating backoff for Client::RateLimitedError retries (15s, 30s, 45s, …). Grows
  # between attempts because calls made *while* limited can extend the restriction — a
  # fixed/short wait would keep re-tripping the exchange's decaying rate counter.
  # Shared by the fetch jobs and Bot::ActionJob so all rate-limit retries use one curve.
  RATE_LIMIT_WAIT = ->(executions) { (15 * executions).seconds }

  def queue_name
    bot = arguments.first
    bot.exchange&.name_id&.to_sym || :default
  end
end
