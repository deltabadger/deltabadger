class LinkStoredFillsToBotOrders < ActiveRecord::Migration[8.1]
  # A bot order and a ledger row are the same event under two ids, and a fill-keyed venue names the
  # order beside the row rather than in it. The matcher only ever ran on INSERT, and a re-sync skips
  # a row it already has — so fixing the matcher reaches nothing already stored, and the bot column
  # would stay empty for the whole history. Relink once, from the order id each fill has been
  # carrying in `raw_data` all along.
  #
  # `update_columns`: this changes no figure, only what a row points at, and bumping `updated_at`
  # would invalidate every cached tracker ledger for nothing.
  def up
    AccountTransaction.where(transaction_id: nil).find_each(batch_size: 500) do |account_transaction|
      raw = account_transaction.raw_data
      order_id = raw['order_id'] if raw.is_a?(Hash)
      next if order_id.blank?

      order = Transaction.find_by(external_id: order_id, exchange_id: account_transaction.exchange_id)
      account_transaction.update_columns(transaction_id: order.id) if order
    end
  end

  def down
    # Not reversible in any useful sense: a link this made is indistinguishable from one the sync
    # made, and dropping both would empty the column again.
  end
end
