# Takes the ids the dashboard grid is showing, in the order the user dragged them into.
#
# The grid is filtered (all / active / inactive / archived), so the payload is often a *subset* of
# the user's bots. Writing 1..n over just those ids would drag every hidden bot along with them
# into an order nobody chose. Instead the payload's sequence is spliced back into exactly the
# slots those bots already occupied, and the whole list is renumbered from there — so bots that
# were off screen keep their place relative to the ones that were on it.
#
# Renumbering everything, rather than redealing the sent bots' existing position values, is also
# what makes the endpoint self-healing. Two bots created in overlapping transactions can share a
# position; redealing `[4, 4]` could never express which of them comes first, and `ordered`'s id
# tie-breaker would settle it the same way forever. A dense rewrite separates them for good.
class Bots::ReordersController < ApplicationController
  before_action :authenticate_user!

  def update
    bots = current_user.bots.ordered.to_a
    dragged = dragged_bots(bots)
    return head :no_content if dragged.empty?

    slots = bots.each_index.select { |i| dragged.include?(bots[i]) }
    slots.each_with_index { |slot, i| bots[slot] = dragged[i] }

    Bot.transaction do
      bots.each_with_index do |bot, i|
        bot.update_column(:position, i + 1) unless bot.position == i + 1
      end
    end

    head :no_content
  end

  private

  # Sanitised on the way in: integers only, no repeats, and nothing the user does not own. Ids
  # that fall away here also fall out of the sequence, so a stranger's id can neither be written
  # nor shift the splice by one.
  def dragged_bots(bots)
    by_id = bots.index_by(&:id)
    Array(params[:ids]).map(&:to_i).uniq.filter_map { |id| by_id[id] }
  end
end
