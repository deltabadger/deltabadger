# The last webhook call that got past a rule's cooldown. One column carries both the replay guard
# (BotSignal#claim_trigger! is a conditional UPDATE on it) and the only trace a call leaves when the
# bot is stopped or the rule is off — "last triggered 2 minutes ago" on the rule widget.
class AddLastTriggeredAtToBotSignals < ActiveRecord::Migration[8.1]
  def change
    add_column :bot_signals, :last_triggered_at, :datetime
  end
end
