# The row app_configs['engine_lease'] records who owned this install last (rust/src/lease.rs). While it
# says "rust", the other engine has not handed its bots back, so this app refuses to start; when it
# records a completed handback, the bots are adopted. The lock itself is taken in 00_engine_lock.rb.
module EngineLease
  module_function

  def check_handover!
    return unless AppConfig.table_exists?

    row = AppConfig.find_by(key: KEY) or return
    lease = begin
      JSON.parse(row.value.to_s)
    rescue JSON::ParserError
      nil
    end
    # Under the wrong keys AppConfig returns the ciphertext itself, which parses as a Hash without
    # "engine". Treat that, like anything else unrecognised, as a reason to stop rather than to guess.
    unless lease.is_a?(Hash) && lease['engine'].is_a?(String)
      raise HeldError, "The engine handover record (app_configs #{KEY}) cannot be read with this install's keys."
    end

    if lease['engine'] != 'none'
      raise HeldError, "The #{lease['engine']} engine #{lease['version']} still owns this data. " \
                       'Run `deltabadger handback` with that engine before starting this app.'
    end
    return unless lease['released_by'] == 'rust'
    raise HeldError, 'The Rust engine released this data without handing its bots back.' unless lease['handed_back'] == true

    adopt_handback!
  end

  # The other engine left every working bot `scheduled` without a job; the repair sweep re-arms each
  # at its next checkpoint. Run it now instead of waiting up to 15 minutes for its recurring slot.
  # Orders still unknown/open are polled explicitly, on ANY bot: one accepted just before the bot was
  # stopped belongs to a bot no sweep visits. The marker goes last; if boot dies in between, the next
  # boot adopts again, and a duplicate poll is harmless.
  def adopt_handback!
    Transaction.submitted.where(external_status: %i[unknown open]).find_each do |order|
      Bot::FetchAndUpdateOrderJob.perform_later(order, update_missed_quote_amount: true)
    end
    Bot::RepairOrphanedBotsJob.perform_later
    AppConfig.where(key: KEY).delete_all
  end
end

Rails.application.config.after_initialize do
  next if Rails.env.test? || defined?(Rails::Console) || ENV['SECRET_KEY_BASE_DUMMY'].present?

  EngineLease.check_handover!
end
