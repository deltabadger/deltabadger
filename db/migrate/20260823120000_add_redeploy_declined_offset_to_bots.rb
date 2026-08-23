# The high-water mark of liquidation proceeds the user has told the bot to forget, so a later sale
# is offered on its own rather than re-offering money already declined.
#
# Its OWN column rather than a transient_data key: the decline is written from a web request, while
# every other transient_data writer holds the exchange semaphore. A read-modify-write on the shared
# JSON blob from outside that lock can drop a sibling key — and the sibling here is the placement
# intent that stops an accepted-but-unrecorded buy from being placed twice.
class AddRedeployDeclinedOffsetToBots < ActiveRecord::Migration[8.1]
  def change
    add_column :bots, :redeploy_declined_offset, :decimal, default: 0, null: false
  end
end
