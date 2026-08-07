# frozen_string_literal: true

namespace :deltabadger do
  namespace :encryption do
    confirmation = 'clear-credentials'
    list = ->(values) { values.presence&.join(', ') || '(none)' }

    # Whether to treat the stored credentials as already being in someone else's hands.
    #
    # Deliberately not "is the CURRENT secret published?". The operator this recovery exists
    # for is often the one who has ALREADY changed their secret — that is why the reset never
    # decrypts. Their current value is strong, so published? is false, while the database
    # still holds credentials encrypted under whatever key wrote them, which may well be the
    # value shipped in this repository. Reading the classification off the current secret
    # tells exactly that operator to relax.
    #
    # So: compromised if the current secret is published, OR if any stored value cannot be
    # decrypted under it. The second proves only that a different key wrote the data, not
    # that the key was published — and that is the right way round to be wrong. Over-warning
    # costs one unnecessary round of revocation; under-warning leaves a stolen exchange key
    # live and trading.
    # Runs in the task, never in Reset: the detection reads encrypted values, and Reset's
    # contract is that it touches none.
    assume_compromised = lambda {
      return true if ENV['PUBLISHED'] == 'yes'
      return true if SecretKeyBaseGuard.published?(Rails.application.secret_key_base)

      EncryptedCredentials::Report.new.data_written_under_another_key?
    }

    desc 'List the credentials that encryption:reset will destroy (read-only)'
    task report: :environment do
      inventory = EncryptedCredentials::Report.new.call
      compromised = assume_compromised.call

      if compromised
        puts "\nAPI keys — REVOKE each of these at the exchange. Deleting them here does not"
        puts 'invalidate them; anyone holding a copy of this database can still trade with them.'
      else
        puts "\nAPI keys stored here — these will be cleared because the encryption key is"
        puts 'changing. Every stored value still decrypts under the current secret, and that'
        puts 'secret is not one published in this repository, so nothing below is known to be'
        puts 'exposed. If you know otherwise, re-run with PUBLISHED=yes.'
      end
      if inventory.api_keys.empty?
        puts '  (none)'
      else
        inventory.api_keys.each { |row| puts "  #{row[:exchange]} (#{row[:key_type]}) — #{row[:user]}" }
      end

      fee_note = compromised ? 'revoke these too' : 'will also be cleared'
      puts "\nFee API keys — #{fee_note}: #{list.call(inventory.fee_api_keys)}"
      puts "Two-factor seed present for: #{list.call(inventory.two_factor_users)}"
      puts "Settings that will reset:    #{list.call(inventory.app_config_keys)}"
      if compromised
        puts 'Every credential stored in those settings must be rotated at its source as well —'
        puts 'SMTP password, Alpaca key and secret, CoinGecko key, market-data token.'
      else
        puts 'Those settings hold credentials too — they will be cleared and need re-entering,'
        puts 'not rotated: SMTP password, Alpaca key and secret, CoinGecko key, market-data token.'
      end

      puts "\nWithdrawal addresses — COPY THESE NOW, they cannot be recovered afterwards:"
      if inventory.withdrawal_addresses.empty?
        puts '  (none)'
      else
        inventory.withdrawal_addresses.each { |row| puts "  #{row[:exchange]} / #{row[:asset]}: #{row[:address]}" }
      end
      puts
    end

    desc 'Clear every encrypted credential so the instance can move to a new SECRET_KEY_BASE'
    task reset: :environment do
      # Computed BEFORE the clear. The reset destroys the very rows this reads, so a check
      # made afterwards would find a spotless database and print the reassuring summary to
      # the operator who most needs the other one.
      compromised = assume_compromised.call

      unless ENV['CONFIRM'] == confirmation
        # Wrapped by hand, with the continuation indent the heredoc's own numbered steps use.
        # Interpolated content is not reflowed by <<~, so a single long string renders as one
        # line in a block where every other line stops at column 82. The indent trails each
        # segment rather than leading the next because Layout/LineContinuationLeadingSpace
        # wants it there; the rendered result is the same either way.
        revoke_step = if compromised
                        "REVOKE every credential it listed, at its source. Do not create\n     " \
                          "the replacements yet — there is nowhere to store them until\n     " \
                          'the app is running again.'
                      else
                        "Nothing here is known to be exposed — this clears credentials\n     " \
                          "because the encryption key is changing, not because anything\n     " \
                          'leaked.'
                      end

        abort <<~MESSAGE

          This deletes every stored API key, every REST API and MCP token, disables
          two-factor authentication, clears withdrawal addresses, resets settings, and stops
          every working bot and rule. It cannot be undone.

          Do these first, in order:
            1. Stop the app. A job a worker has already picked up cannot be cancelled and
               would keep trading while this runs.
            2. Back up /app/storage — with the app stopped, so the copy is consistent.
            3. Run `deltabadger:encryption:report`. Copy your withdrawal addresses.
            4. #{revoke_step}

          Exposure is judged from two things: whether the secret you are running now is one
          published in this repository, and whether anything stored still decrypts under it.
          Neither can see a key you have already replaced and discarded. If you know the key
          this data was written under was published, re-run with PUBLISHED=yes as well and
          every message here will say so.

          Re-run with:  CONFIRM=#{confirmation} bin/rails deltabadger:encryption:reset

        MESSAGE
      end

      ibkr = ApiKey.joins(:exchange).where(exchanges: { type: 'Exchanges::Ibkr' }).exists?
      if ibkr && ENV['CONFIRM_IBKR'] != 'yes'
        abort <<~MESSAGE

          This instance holds Interactive Brokers credentials. Unlike an exchange API key,
          IBKR's RSA material is generated by this app and stored only here, so it cannot
          be re-entered and cannot be replaced in advance. After this you must run the
          connect wizard again to generate a fresh key pair, register it in IBKR's portal,
          and wait for IBKR to activate it — which has taken days. Everything else here
          takes minutes to restore.

          If you accept that, re-run with CONFIRM_IBKR=yes as well.

        MESSAGE
      end

      summary = EncryptedCredentials::Reset.new.call

      puts "\nCleared:"
      puts "  API keys deleted:            #{summary.api_keys_deleted}"
      puts "  Fee API keys deleted:        #{summary.fee_api_keys_deleted}"
      puts "  Settings cleared:            #{summary.app_configs_deleted}"
      puts "  Withdrawal addresses:        #{summary.withdrawal_addresses_cleared}"
      puts "  Two-factor seed cleared for: #{summary.two_factor_disabled} user(s)"
      # Not in the report's inventory — it lists encrypted attributes, and these are not one.
      # Printing the count here is the only place the operator sees that the REST API token
      # and every MCP connection are gone, which is what step 8 asks them to restore.
      puts "  REST API and MCP tokens:     #{summary.oauth_tokens_deleted}"
      puts "  Bots stopped:                #{summary.bots_stopped}"
      puts "  Rules stopped:               #{summary.rules_stopped}"

      if compromised
        puts "\nThose credentials should ALREADY have been REVOKED at their source before this"
        puts 'ran — deleting them here never invalidated them, and a copy of the old database'
        puts 'still contains them. If you have not done that yet, do it now.'
      else
        puts "\nNothing here was known to be exposed, so nothing needed revoking first — this"
        puts 'only cleared credentials because the encryption key is changing.'
      end

      # The operator is in a terminal here, often with no README open, and this is the last
      # thing they read before acting. It has to carry both halves of the rotation: the
      # secret can live in .env.docker, in a compose file, or — on a default install, where
      # .env.docker ships blank — in /app/storage/.secrets, which is reused verbatim unless
      # it is deleted. And the container has to be recreated, because compose bakes the
      # environment in at create time.
      puts "\nNext: point this install at a new key. Blank the SECRET_KEY_BASE value in"
      puts '.env.docker and delete the generated key, which is a separate file:'
      puts '  docker compose run --rm --no-deps deltabadger rm -f /app/storage/.secrets'
      puts 'On a default install that file is where your key actually is, and a new one is'
      puts 'written only when it is absent. If your value comes from a compose file instead,'
      puts 'put a freshly generated one there (openssl rand -hex 64) rather than blanking it:'
      puts 'that file runs two services off one volume, and each would generate its own.'
      puts 'Then recreate the container, which is what picks up the change:'
      puts '  docker compose up -d --force-recreate'
      puts 'Starting the stopped container instead brings the old value back with it.'
      puts "\nIssue the replacement credentials, re-enable two-factor, re-issue your REST"
      puts 'API token, reconnect your MCP clients, and restart your bots and rules — until'
      puts "the app is running there is nowhere to store the new credentials.\n\n"

      if summary.ibkr_credentials_destroyed
        puts 'Interactive Brokers: once the app is back up, run the connect wizard again to'
        puts 'generate a fresh key pair, register it in their portal, and wait for IBKR to'
        puts "activate it.\n\n"
      end
    end
  end
end
