# Clears every encrypted credential in the install.
#
# Never decrypts: every write goes through update_all/delete_all on raw columns. That is
# what lets this run on an instance whose secret_key_base has already been changed — where
# the stored ciphertext is unreadable, two-factor sign-in is broken and the operator cannot
# reach the UI at all. Reading any encrypted attribute here would reintroduce exactly that
# dependency, and support_unencrypted_data means a failed read returns ciphertext rather
# than raising, so the damage would be silent.
#
# Requires a stopped stack. Cancelling a bot's scheduled jobs cannot stop an execution a
# worker has already claimed — Automation::Schedulable#cancel_solid_queue_jobs removes
# scheduled, ready and blocked rows, but never ClaimedExecution — and standalone mode runs
# Solid Queue inside Puma, so the app being up means a worker is up.
module EncryptedCredentials
  class Reset
    Summary = Struct.new(
      :bots_stopped, :rules_stopped, :withdrawal_addresses_cleared,
      :api_keys_deleted, :fee_api_keys_deleted, :app_configs_deleted,
      :two_factor_disabled, :ibkr_credentials_destroyed,
      keyword_init: true
    )

    def call
      had_ibkr = ApiKey.joins(:exchange).where(exchanges: { type: 'Exchanges::Ibkr' }).exists?
      stopped_bots = stop_bots

      # Scoped to the primary database on purpose: job cancellation above writes to the
      # queue database on a different connection, which a single transaction cannot span.
      ApplicationRecord.transaction do
        Summary.new(
          bots_stopped: stopped_bots,
          rules_stopped: stop_rules,
          withdrawal_addresses_cleared: clear_withdrawal_addresses,
          api_keys_deleted: delete_api_keys,
          fee_api_keys_deleted: FeeApiKey.delete_all,
          app_configs_deleted: AppConfig.delete_all,
          two_factor_disabled: disable_two_factor,
          ibkr_credentials_destroyed: had_ibkr
        )
      end
    end

    private

    # Bots carry no encrypted attributes, and cancel_scheduled_action_jobs matches Solid
    # Queue rows by job class and record, so loading them here decrypts nothing. The status
    # write is update_all rather than Bot#stop because Bot#stop runs set_missed_quote_amount,
    # full validations, activity logging and broadcasts spread across a dozen concerns. None
    # of that reaches a credential today, but this service's contract is that it touches no
    # encrypted attribute at all, and that contract can't rest on every current and future
    # bot callback staying credential-free. The same discipline is why stop_rules below is
    # update_all too, not Rule#stop: Rules::Withdrawal#stop is an update! that would raise on
    # an already-nulled address.
    #
    # Automation::Schedulable is included by the DCA subclasses, not by Bot — Bots::Signal
    # is a perfectly ordinary working bot without it. Calling the method unconditionally
    # would raise NoMethodError and abort the whole recovery on exactly the installs that
    # need it. Every working bot is still stopped; only the schedulable ones have jobs to
    # cancel.
    def stop_bots
      bots = Bot.working.to_a
      bots.select { |bot| bot.respond_to?(:cancel_scheduled_action_jobs) }
          .each(&:cancel_scheduled_action_jobs)
      Bot.where(id: bots.map(&:id)).update_all(status: Bot.statuses[:stopped], stopped_at: Time.current)
    end

    def stop_rules
      Rule.working.update_all(status: Rule.statuses[:stopped])
    end

    def clear_withdrawal_addresses
      Rule.where(type: 'Rules::Withdrawal').where.not(address: nil).update_all(address: nil)
    end

    # account_transactions has a real foreign key to api_keys (db/schema.rb:504), so the
    # references are cleared explicitly rather than leaning on dependent: :nullify, which
    # would need destroy callbacks and put model code back in the decryption path.
    def delete_api_keys
      AccountTransaction.where.not(api_key_id: nil).update_all(api_key_id: nil)
      ApiKey.delete_all
    end

    def disable_two_factor
      User.with_two_factor_material.update_all(otp_secret_key: nil, otp_module: User.otp_modules[:disabled])
    end
  end
end
