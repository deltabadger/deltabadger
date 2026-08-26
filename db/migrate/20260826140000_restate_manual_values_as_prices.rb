# The record holds amounts and prices, never a value. A value stated by hand under the old rule is
# restated as the price it implied, so the row reads the same after the change as before it.
#
# The rows the retired "Fix" modal wrote go with it. They were never the venue's: a deposit or a
# withdrawal the modal proposed so the history would match the balance, dated the moment it was
# accepted, with no price. The page now reads that same difference from the balance itself and
# says so on the row, without writing anything — so what the modal wrote is a second, wrongly
# dated copy of an assumption the page already makes. A link a user drew to one of them is
# undrawn first, or the surviving leg would point at nothing.
class RestateManualValuesAsPrices < ActiveRecord::Migration[8.1]
  def up
    users = Set.new

    # Over the SIZE of the row: a split is one signed net delta, and a value stated on a reverse
    # split was stated on what moved, not on its sign. Only a row of nothing has no price to imply.
    AccountTransaction.where("manual_values LIKE '%fiat_value%'").find_each do |row|
      value = row.manual_values['fiat_value'].presence&.to_d
      amount = row.base_amount.to_d.abs
      price = value / amount if value && amount.positive?
      row.update_column(:manual_values, price ? { 'price' => price.to_s } : {})
      users << row.user_id
    end

    retired = AccountTransaction.where("tx_id LIKE 'manual-%' AND raw_data LIKE '%\"source\":\"manual\"%'")
    users.merge(retired.distinct.pluck(:user_id))
    AccountTransaction.where(linked_transaction_id: retired.select(:id)).update_all(linked_transaction_id: nil)
    retired.delete_all

    users.each do |user_id|
      PortfolioSnapshot::BackfillJob.perform_later(user_id)
      Tracker::LedgerJob.perform_later(user_id)
    end
  end

  # The release before this one reads only `fiat_value`: a stated price left under its new key
  # would vanish from the tracker, the ledger and the tax report. It goes back as the value it
  # implies. The retired modal's rows are not restored — they were assumptions, not records.
  def down
    AccountTransaction.where("manual_values LIKE '%price%'").find_each do |row|
      price = row.manual_values['price'].presence&.to_d
      value = price * row.base_amount.to_d.abs if price
      row.update_column(:manual_values, value ? { 'fiat_value' => value.to_s } : {})
    end
  end
end
