# The /hook/:token receiver has no route yet, so nothing breaks by rotating these values in
# place today. Once it is wired up, the token becomes a trade-triggering credential, and
# rotating it stops being free.
#
# Raw SQL throughout, not the BotSignal model: the app models keep changing, and this
# migration needs to keep running unmodified on every self-hosted install for years.
class RotateBotSignalTokens < ActiveRecord::Migration[8.1]
  BATCH_SIZE = 1000

  def up
    rotated = 0

    # Two independent logging paths can print a token, and both need closing. Bare quote/execute
    # calls resolve through Migration#method_missing, which logs its arguments via
    # say_with_time; suppress_messages keeps that path silent, reporting only a final count.
    # quote and execute are also overridden locally below, bypassing method_missing's logging
    # entirely, so neither call site here can reach the verbose migration log even if
    # suppress_messages is ever removed. Neither guard touches ActiveRecord's own adapter
    # logging, though: connection.execute fires the sql.active_record notification regardless,
    # and at :debug (development's default, which a debug build of the desktop app runs under)
    # that logs the full UPDATE statement, token included. logger.silence raises the effective
    # level past :debug for the duration, closing that path too.
    suppress_messages do
      ActiveRecord::Base.logger&.silence do
        used_tokens = Set.new(select_values('SELECT token FROM bot_signals'))
        last_id = 0

        loop do
          ids = select_values(<<~SQL.squish).map(&:to_i)
            SELECT id FROM bot_signals WHERE id > #{last_id} ORDER BY id LIMIT #{BATCH_SIZE}
          SQL
          break if ids.empty?

          ids.each do |id|
            token = unused_token(used_tokens)
            used_tokens << token
            execute("UPDATE bot_signals SET token = #{quote(token)} WHERE id = #{id}")
            rotated += 1
          end
          last_id = ids.last
        end
      end
    end

    say "Rotated #{rotated} token(s)"
  end

  def down
    raise ActiveRecord::IrreversibleMigration
  end

  private

  # Regenerates on collision rather than trusting 256 bits of entropy blindly: the column
  # carries a unique index, and a duplicate would otherwise abort the migration outright.
  def unused_token(used_tokens)
    loop do
      token = SecureRandom.urlsafe_base64(32)
      return token unless used_tokens.include?(token)
    end
  end

  # quote and execute are defined locally rather than delegated through method_missing, so
  # neither call site here can end up in the verbose migration log even if suppress_messages is
  # ever removed above -- method_missing (and its say_with_time wrapper) is only reached for
  # methods the class does not already implement.
  def quote(value)
    connection.quote(value)
  end

  def execute(sql)
    connection.execute(sql)
  end
end
