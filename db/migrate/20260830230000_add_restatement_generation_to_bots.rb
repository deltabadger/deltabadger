# A bot's metrics are cached for 30 days and computed from a position a corporate action moves.
# Deleting those keys when one lands is not enough on its own: a walk that read the ledger just
# BEFORE the split was stored can finish just after, and write its pre-split answer back over the
# hole. That hash carries no restatement, so nothing downstream knows to distrust it.
#
# The generation makes the old and new answers live at different keys. A writer that read the old
# ledger computed the old key and can only ever write there; the key everything reads next is one
# nothing pre-split can reach.
class AddRestatementGenerationToBots < ActiveRecord::Migration[8.1]
  def change
    add_column :bots, :restatement_generation, :integer, default: 0, null: false
  end
end
