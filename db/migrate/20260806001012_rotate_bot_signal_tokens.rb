# The /hook/:token receiver has no route yet, so nothing breaks by rotating these values in
# place today. Once it is wired up, the token becomes a trade-triggering credential, and
# rotating it stops being free.
#
# Raw SQL throughout, not the BotSignal model: the app models keep changing, and this
# migration needs to keep running unmodified on every self-hosted install for years.
class RotateBotSignalTokens < ActiveRecord::Migration[8.1]
  def up
    used_tokens = select_values('SELECT token FROM bot_signals')

    select_values('SELECT id FROM bot_signals').each do |id|
      token = unused_token(used_tokens)
      used_tokens << token
      execute("UPDATE bot_signals SET token = #{quote(token)} WHERE id = #{id.to_i}")
    end
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
end
