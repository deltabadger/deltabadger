# The bots list has always been sorted by `label` — a Haikunator name the user never picked. This
# gives them a hand-picked order instead, dragged on the dashboard.
#
# Backfilled to the order users see today, so nothing moves on the deploy. `(label, id)` rather
# than `label` alone: labels are unique per user in practice but nothing enforces it, and a tie
# would otherwise backfill non-deterministically.
#
# Starts at 1, never 0: the column default of 0 is the "unset" sentinel `Bot#assign_position`
# looks for, so a backfilled row must never look unset.
#
# Self-contained raw SQL, no Bot model — a historical migration that depends on head-of-branch
# code breaks fresh installs migrating from zero.
class AddPositionToBots < ActiveRecord::Migration[8.1]
  def up
    add_column :bots, :position, :integer, null: false, default: 0

    # ponytail: correlated subquery, so O(n^2) per user. A container holds one user with tens of
    # bots; if that ever stops being true this becomes a window function.
    execute <<~SQL.squish
      UPDATE bots SET position = 1 + (
        SELECT COUNT(*) FROM bots AS earlier
        WHERE earlier.user_id IS bots.user_id
          AND (earlier.label < bots.label OR (earlier.label = bots.label AND earlier.id < bots.id))
      )
    SQL
  end

  def down
    remove_column :bots, :position
  end
end
